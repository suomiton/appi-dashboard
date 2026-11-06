//! One dashboard refresh: every metric and query the two tabs need.

use std::collections::HashMap;

use chrono::{DateTime, Duration, Utc};

use crate::azure::discovery::AppInstance;
use crate::azure::http::AzError;
use crate::azure::logs::Table;
use crate::azure::metrics::{Aggregation, Interval, MetricSeries, MetricsRequest};
use crate::data::backend::Backend;
use crate::data::catalog::{
    ALERT_METRICS, AVERAGE_METRICS, COUNT_METRICS, MetricSpec, SeriesId, series_for,
};
use crate::data::kql;
use crate::data::time::{TimeRange, window};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum View {
    Overview,
    Failures,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Scalars {
    pub users_5m: Option<f64>,
    pub users_range: Option<f64>,
    pub sessions_range: Option<f64>,
    pub servers_10m: Option<f64>,
    pub response_p95_ms: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AlertWindow {
    pub failed_5m: f64,
    pub server_exceptions_5m: f64,
    pub dependency_failures_5m: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TopItem {
    pub name: String,
    pub detail: String,
    pub n: f64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FailureTops {
    pub endpoints: Vec<TopItem>,
    pub exceptions: Vec<TopItem>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Series {
    pub points: Vec<(DateTime<Utc>, Option<f64>)>,
}

impl Series {
    pub fn values(&self) -> Vec<Option<f64>> {
        self.points.iter().map(|(_, v)| *v).collect()
    }

    pub fn latest(&self) -> Option<f64> {
        self.points.iter().rev().find_map(|(_, v)| *v)
    }

    pub fn max(&self) -> Option<f64> {
        self.points.iter().filter_map(|(_, v)| *v).reduce(f64::max)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Source {
    CountMetrics,
    AverageMetrics,
    CountTotals,
    AverageTotals,
    AlertWindow,
    Scalars,
    UsersSessions,
    Tops,
}

pub const ALL_SOURCES: [Source; 8] = [
    Source::CountMetrics,
    Source::AverageMetrics,
    Source::CountTotals,
    Source::AverageTotals,
    Source::AlertWindow,
    Source::Scalars,
    Source::UsersSessions,
    Source::Tops,
];

impl Source {
    /// The series a source fills (`totals == false`) or the totals it
    /// fills (`totals == true`).
    pub fn covers(self) -> (&'static [crate::data::catalog::MetricSpec], bool) {
        match self {
            Source::CountMetrics => (COUNT_METRICS, false),
            Source::AverageMetrics => (AVERAGE_METRICS, false),
            Source::CountTotals => (COUNT_METRICS, true),
            Source::AverageTotals => (AVERAGE_METRICS, true),
            _ => (&[], false),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct DashboardUpdate {
    pub series: HashMap<SeriesId, Series>,
    pub totals: HashMap<SeriesId, f64>,
    pub scalars: Option<Scalars>,
    pub alert_window: Option<AlertWindow>,
    pub tops: Option<FailureTops>,
    pub errors: Vec<(Source, AzError)>,
    /// Sources whose call succeeded. A successful source that omits a
    /// series or total means "no data now", not "keep the old value".
    pub succeeded: Vec<Source>,
}

fn request(
    inst: &AppInstance,
    group: &[MetricSpec],
    aggregation: Aggregation,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    interval: Interval,
) -> MetricsRequest {
    MetricsRequest {
        resource_id: inst.resource_id.clone(),
        metric_names: group.iter().map(|m| m.metric.to_string()).collect(),
        aggregation,
        start,
        end,
        interval,
        split_by: None,
    }
}

/// Fetches everything the dashboard shows. Each source fails independently:
/// a failed call becomes an entry in `errors` and the rest still fills in.
pub async fn refresh_dashboard(
    backend: &dyn Backend,
    inst: &AppInstance,
    range: TimeRange,
    now: DateTime<Utc>,
) -> DashboardUpdate {
    let w = window(range, now);
    let alert_start = w.end - Duration::minutes(10);
    let iso = range.iso();
    let scalars_kql = kql::summary_scalars();
    let series_kql = kql::users_sessions_series(range);
    let tops_kql = kql::failure_tops();

    let count = backend.metrics(request(
        inst,
        COUNT_METRICS,
        Aggregation::Count,
        w.start,
        w.end,
        range.grain(),
    ));
    let average = backend.metrics(request(
        inst,
        AVERAGE_METRICS,
        Aggregation::Average,
        w.start,
        w.end,
        range.grain(),
    ));
    let count_totals = backend.metrics(request(
        inst,
        COUNT_METRICS,
        Aggregation::Count,
        w.start,
        w.end,
        Interval::Full,
    ));
    let average_totals = backend.metrics(request(
        inst,
        AVERAGE_METRICS,
        Aggregation::Average,
        w.start,
        w.end,
        Interval::Full,
    ));
    let alert = backend.metrics(request(
        inst,
        ALERT_METRICS,
        Aggregation::Count,
        alert_start,
        w.end,
        Interval::PT1M,
    ));
    let scalars = backend.query(&inst.app_id, &scalars_kql, iso);
    let users_sessions = backend.query(&inst.app_id, &series_kql, iso);
    let tops = backend.query(&inst.app_id, &tops_kql, "PT15M");

    let (count, average, count_totals, average_totals, alert, scalars, users_sessions, tops) = tokio::join!(
        count,
        average,
        count_totals,
        average_totals,
        alert,
        scalars,
        users_sessions,
        tops
    );

    let mut u = DashboardUpdate::default();
    for (source, result) in [
        (Source::CountMetrics, count),
        (Source::AverageMetrics, average),
    ] {
        match result {
            Ok(list) => {
                for m in list {
                    if let Some(id) = series_for(&m.name) {
                        u.series.insert(id, Series { points: m.points });
                    }
                }
            }
            Err(e) => u.errors.push((source, e)),
        }
    }
    match count_totals {
        Ok(list) => {
            for m in list {
                if let Some(id) = series_for(&m.name) {
                    let values: Vec<f64> = m.points.iter().filter_map(|(_, v)| *v).collect();
                    if !values.is_empty() {
                        u.totals.insert(id, values.iter().sum());
                    }
                }
            }
        }
        Err(e) => u.errors.push((Source::CountTotals, e)),
    }
    match average_totals {
        Ok(list) => {
            for m in list {
                if let (Some(id), Some(v)) =
                    (series_for(&m.name), m.points.iter().find_map(|(_, v)| *v))
                {
                    u.totals.insert(id, v);
                }
            }
        }
        Err(e) => u.errors.push((Source::AverageTotals, e)),
    }
    match alert {
        Ok(list) => u.alert_window = Some(alert_window(&list, w.end - Duration::minutes(5))),
        Err(e) => u.errors.push((Source::AlertWindow, e)),
    }
    match scalars {
        Ok(t) => u.scalars = Some(parse_scalars(&t)),
        Err(e) => u.errors.push((Source::Scalars, e)),
    }
    match users_sessions {
        Ok(t) => {
            let (users, sessions) = parse_users_sessions(&t);
            u.series.insert(SeriesId::Users, users);
            u.series.insert(SeriesId::Sessions, sessions);
        }
        Err(e) => u.errors.push((Source::UsersSessions, e)),
    }
    match tops {
        Ok(t) => u.tops = Some(parse_tops(&t)),
        Err(e) => u.errors.push((Source::Tops, e)),
    }
    u.succeeded = ALL_SOURCES
        .iter()
        .copied()
        .filter(|s| !u.errors.iter().any(|(e, _)| e == s))
        .collect();
    u
}

fn alert_window(list: &[MetricSeries], since: DateTime<Utc>) -> AlertWindow {
    let mut w = AlertWindow::default();
    for m in list {
        let sum: f64 = m
            .points
            .iter()
            .filter(|(at, _)| *at >= since)
            .filter_map(|(_, v)| *v)
            .sum();
        match series_for(&m.name) {
            Some(SeriesId::RequestsFailed) => w.failed_5m += sum,
            Some(SeriesId::ExceptionsServer) => w.server_exceptions_5m += sum,
            Some(SeriesId::DependenciesFailed) => w.dependency_failures_5m += sum,
            _ => {}
        }
    }
    w
}

fn parse_scalars(t: &Table) -> Scalars {
    Scalars {
        users_5m: t.f64(0, "users_5m"),
        users_range: t.f64(0, "users_range"),
        sessions_range: t.f64(0, "sessions_range"),
        servers_10m: t.f64(0, "servers_10m"),
        response_p95_ms: t.f64(0, "response_p95_ms"),
    }
}

fn parse_users_sessions(t: &Table) -> (Series, Series) {
    let mut users = Series::default();
    let mut sessions = Series::default();
    for row in 0..t.len() {
        if let Some(at) = t.time(row, "timestamp") {
            users.points.push((at, t.f64(row, "users")));
            sessions.points.push((at, t.f64(row, "sessions")));
        }
    }
    (users, sessions)
}

fn parse_tops(t: &Table) -> FailureTops {
    let mut tops = FailureTops::default();
    for row in 0..t.len() {
        let item = TopItem {
            name: t.str(row, "name").unwrap_or_default().to_string(),
            detail: t.str(row, "detail").unwrap_or_default().to_string(),
            n: t.f64(row, "n").unwrap_or(0.0),
        };
        match t.str(row, "section") {
            Some("endpoint") => tops.endpoints.push(item),
            Some("exception") => tops.exceptions.push(item),
            _ => {}
        }
    }
    let by_n = |a: &TopItem, b: &TopItem| b.n.total_cmp(&a.n);
    tops.endpoints.sort_by(by_n);
    tops.exceptions.sort_by(by_n);
    tops
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::azure::metrics::{Aggregation, Interval};
    use crate::data::backend::fake::{FakeBackend, table};
    use crate::data::kql;
    use chrono::TimeZone;
    use serde_json::json;

    pub(crate) fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 4, 13, 6, 42).unwrap()
    }

    pub(crate) fn demo_instance() -> AppInstance {
        crate::data::demo::demo_instances().remove(0)
    }

    /// Metric responses: every requested metric gets a series of 1.0 values
    /// except requests/failed, which carries the shapes the assertions use.
    pub(crate) fn fixture_backend() -> FakeBackend {
        FakeBackend::new()
            .with_metrics(|req| {
                let step = match req.interval {
                    Interval::PT1M => 1,
                    Interval::PT5M => 5,
                    Interval::PT15M => 15,
                    Interval::PT30M => 30,
                    Interval::Full => 0,
                };
                Ok(req
                    .metric_names
                    .iter()
                    .map(|name| {
                        let points = if step == 0 {
                            let v = if name == "requests/failed" { 24.0 } else { 1.0 };
                            vec![(req.start, Some(v))]
                        } else {
                            let n = (req.end - req.start).num_minutes() / step;
                            (0..n)
                                .map(|i| {
                                    let at = req.start + chrono::Duration::minutes(i * step);
                                    let v = if name == "requests/failed" && step == 1 {
                                        // last five minutes: 2+3+2+3+2 = 12
                                        [0., 0., 0., 0., 0., 2., 3., 2., 3., 2.]
                                            .get(i as usize)
                                            .copied()
                                            .unwrap_or(0.)
                                    } else {
                                        1.0
                                    };
                                    (at, Some(v))
                                })
                                .collect()
                        };
                        MetricSeries {
                            name: name.clone(),
                            dimension: None,
                            points,
                        }
                    })
                    .collect())
            })
            .with_query(
                kql::summary_scalars(),
                Ok(table(
                    &[
                        "users_5m",
                        "users_range",
                        "sessions_range",
                        "servers_10m",
                        "response_p95_ms",
                    ],
                    vec![json!([165, 343, 360, 2, 480.0])],
                )),
            )
            .with_query(
                kql::users_sessions_series(TimeRange::H4),
                Ok(table(
                    &["timestamp", "users", "sessions"],
                    vec![
                        json!(["2026-11-04T09:05:00Z", 10, 12]),
                        json!(["2026-11-04T09:10:00Z", 11, 14]),
                    ],
                )),
            )
            .with_query(
                kql::failure_tops(),
                Ok(table(
                    &["section", "name", "detail", "n"],
                    vec![
                        json!(["endpoint", "POST /api/orders", "", 12]),
                        json!(["endpoint", "GET /api/reports/export", "", 2]),
                        json!(["exception", "SqlException", "Execution Timeout Expired", 9]),
                    ],
                )),
            )
    }

    #[tokio::test]
    async fn refresh_collects_all_sources() {
        let b = fixture_backend();
        let u = refresh_dashboard(&b, &demo_instance(), TimeRange::H4, now()).await;
        assert!(u.errors.is_empty(), "{:?}", u.errors);
        assert_eq!(u.succeeded.len(), 8, "every source reports success");
        assert_eq!(u.series[&SeriesId::RequestsFailed].points.len(), 48);
        assert_eq!(u.totals[&SeriesId::RequestsFailed], 24.0);
        assert_eq!(u.alert_window.as_ref().unwrap().failed_5m, 12.0);
        let s = u.scalars.as_ref().unwrap();
        assert_eq!(s.users_5m, Some(165.0));
        assert_eq!(s.servers_10m, Some(2.0));
        assert_eq!(u.series[&SeriesId::Sessions].points[1].1, Some(14.0));
        let tops = u.tops.as_ref().unwrap();
        assert_eq!(tops.endpoints[0].name, "POST /api/orders");
        assert_eq!(tops.endpoints.len(), 2);
        assert_eq!(tops.exceptions[0].detail, "Execution Timeout Expired");
        let calls = b.calls();
        assert!(
            calls
                .iter()
                .any(|c| c.starts_with("metrics:count:PT5M:requests/count"))
        );
        assert!(calls.iter().any(|c| c.starts_with("metrics:average:FULL:")));
        assert!(
            calls
                .iter()
                .any(|c| c.starts_with("metrics:count:PT1M:requests/failed"))
        );
        assert!(calls.iter().any(|c| c.starts_with("query:PT15M:")));
        let _ = Aggregation::Count;
    }

    #[tokio::test]
    async fn every_refresh_fetches_tops() {
        let b = fixture_backend();
        let u = refresh_dashboard(&b, &demo_instance(), TimeRange::H4, now()).await;
        assert!(u.tops.is_some());
        assert!(b.calls().iter().any(|c| c.contains("let eps")));
    }

    #[tokio::test]
    async fn partial_failure_keeps_other_parts() {
        let b = fixture_backend().with_metrics(|req| {
            if req.aggregation == Aggregation::Average {
                Err(AzError::Http {
                    status: 500,
                    message: "boom".into(),
                })
            } else {
                Ok(vec![MetricSeries {
                    name: "requests/failed".into(),
                    dimension: None,
                    points: vec![(req.start, Some(1.0))],
                }])
            }
        });
        let u = refresh_dashboard(&b, &demo_instance(), TimeRange::H4, now()).await;
        assert!(u.errors.iter().any(|(s, _)| *s == Source::AverageMetrics));
        assert!(u.errors.iter().any(|(s, _)| *s == Source::AverageTotals));
        assert!(u.series.contains_key(&SeriesId::RequestsFailed));
        assert!(!u.series.contains_key(&SeriesId::ServerResponse));
    }
}
