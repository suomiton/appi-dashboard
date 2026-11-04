//! Deterministic fake data for `--demo` and for tests.
//!
//! Values depend only on the metric name and the bucket's timestamp, so the
//! same window always produces the same numbers. All names are made up.

use chrono::{DateTime, Duration, Utc};
use serde_json::{Value, json};

use crate::azure::discovery::AppInstance;
use crate::azure::http::AzError;
use crate::azure::logs::Table;
use crate::azure::metrics::{Interval, MetricSeries, MetricsRequest};
use crate::data::backend::Backend;
use crate::data::kql;
use crate::data::time::TimeRange;

const ZERO: &str = "00000000-0000-0000-0000-000000000000";
const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
const MIB: f64 = 1024.0 * 1024.0;

pub fn demo_instances() -> Vec<AppInstance> {
    ["demo-app-prod", "demo-app-staging"]
        .iter()
        .enumerate()
        .map(|(i, name)| AppInstance {
            name: name.to_string(),
            resource_id: format!(
                "/subscriptions/{ZERO}/resourceGroups/demo-rg/providers/microsoft.insights/components/{name}"
            ),
            resource_group: "demo-rg".into(),
            subscription_id: ZERO.into(),
            subscription_name: "Demo Subscription".into(),
            tenant_id: ZERO.into(),
            app_id: format!("{i}0000000-0000-0000-0000-000000000000"),
            location: "westeurope".into(),
        })
        .collect()
}

pub struct DemoBackend {
    seed: u64,
}

impl Default for DemoBackend {
    fn default() -> Self {
        Self::new()
    }
}

/// A stable pseudo-random value in [0, 1) for (seed, key, i).
fn noise(seed: u64, key: &str, i: i64) -> f64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in key.bytes() {
        h = (h ^ b as u64).wrapping_mul(0x100000001b3);
    }
    let mut z = seed ^ h ^ (i as u64).wrapping_mul(0x9E3779B97F4A7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
    z ^= z >> 31;
    (z >> 11) as f64 / (1u64 << 53) as f64
}

fn step_minutes(interval: Interval, start: DateTime<Utc>, end: DateTime<Utc>) -> i64 {
    match interval {
        Interval::PT1M => 1,
        Interval::PT5M => 5,
        Interval::PT15M => 15,
        Interval::PT30M => 30,
        Interval::Full => (end - start).num_minutes().max(1),
    }
}

impl DemoBackend {
    pub fn new() -> Self {
        Self { seed: 0x5eed }
    }

    /// One bucket of `metric` starting at `at`, `step` minutes long.
    fn bucket(
        &self,
        metric: &str,
        at: DateTime<Utc>,
        end: DateTime<Utc>,
        step: i64,
    ) -> Option<f64> {
        let i = at.timestamp() / 60 / step;
        let n = noise(self.seed, metric, i);
        let n2 = noise(self.seed ^ 1, metric, i);
        let to_end = (end - at).num_minutes();
        let recent = to_end <= 10;
        let s = step as f64;
        Some(match metric {
            "requests/count" => 412.0 * s * (0.85 + 0.3 * n),
            "requests/failed" => {
                if step == 1 && to_end <= 5 {
                    [2.0, 3.0, 2.0, 3.0, 2.0][((to_end - 1).clamp(0, 4)) as usize]
                } else if recent {
                    (2.0 * s).max(1.0)
                } else if n < 0.05 {
                    1.0
                } else {
                    0.0
                }
            }
            "exceptions/server" => {
                if recent {
                    3.0
                } else if n < 0.15 {
                    (n2 * 6.0).floor()
                } else {
                    0.0
                }
            }
            "exceptions/browser" => {
                if n < 0.2 {
                    1.0 + (n2 * 15.0).floor()
                } else {
                    (n2 * 3.0).floor()
                }
            }
            "dependencies/failed" => {
                if n < 0.25 {
                    1.0 + (n2 * 4.0).floor()
                } else {
                    0.0
                }
            }
            "availabilityResults/count" => s,
            "requests/duration" => 110.0 + 30.0 * n + if n2 < 0.05 { 900.0 } else { 0.0 },
            "browserTimings/totalDuration" => {
                560.0 + 300.0 * n + if n2 < 0.08 { 800.0 } else { 0.0 }
            }
            "browserTimings/networkDuration" => 120.0 + 60.0 * n,
            "browserTimings/processingDuration" => 300.0 + 80.0 * n,
            "browserTimings/sendDuration" => 60.0 + 40.0 * n,
            "performanceCounters/processCpuPercentage" => {
                0.25 + 0.2 * n + if n2 < 0.07 { 3.0 } else { 0.0 }
            }
            "performanceCounters/memoryAvailableBytes" => {
                (11.7 + 0.25 * ((i as f64) / 7.0).sin() + 0.05 * n) * GIB
            }
            "performanceCounters/processIOBytesPerSecond" => {
                12.9 * MIB * (0.7 + 0.6 * n) + if n2 < 0.02 { 500.0 * MIB } else { 0.0 }
            }
            "availabilityResults/availabilityPercentage" => 100.0,
            _ => return None,
        })
    }

    /// The whole-window value, matching the mockup's legend numbers.
    fn full(&self, metric: &str, minutes: i64) -> Option<f64> {
        let m = minutes as f64;
        Some(match metric {
            "requests/count" => 412.0 * m,
            "requests/failed" => 24.0,
            "exceptions/server" => 30.0,
            "exceptions/browser" => 60.0,
            "dependencies/failed" => 45.0,
            "availabilityResults/count" => m,
            "requests/duration" => 119.48,
            "browserTimings/totalDuration" => 565.0,
            "browserTimings/networkDuration" => 148.95,
            "browserTimings/processingDuration" => 335.5,
            "browserTimings/sendDuration" => 80.28,
            "performanceCounters/processCpuPercentage" => 0.3137,
            "performanceCounters/memoryAvailableBytes" => 11.72 * GIB,
            "performanceCounters/processIOBytesPerSecond" => 12.9 * MIB,
            "availabilityResults/availabilityPercentage" => 100.0,
            _ => return None,
        })
    }

    fn users_sessions(&self, range: TimeRange) -> Table {
        let g = range.grain_minutes();
        let now = Utc::now();
        let end = now - Duration::seconds(now.timestamp() % (g * 60));
        let n = range.minutes() / g;
        let rows = (0..n)
            .map(|k| {
                let at = end - Duration::minutes((n - k) * g);
                let i = at.timestamp() / 60 / g;
                let users = 6.0 + (noise(self.seed, "users", i) * 12.0).floor();
                let sessions = users + (noise(self.seed, "sessions", i) * 3.0).floor();
                json!([at.to_rfc3339(), users, sessions])
            })
            .collect();
        table(&["timestamp", "users", "sessions"], rows)
    }
}

fn table(columns: &[&str], rows: Vec<Value>) -> Table {
    Table {
        columns: columns.iter().map(|c| c.to_string()).collect(),
        rows: rows
            .into_iter()
            .map(|r| r.as_array().cloned().unwrap_or_default())
            .collect(),
    }
}

#[async_trait::async_trait]
impl Backend for DemoBackend {
    async fn instances(&self) -> Result<Vec<AppInstance>, AzError> {
        Ok(demo_instances())
    }

    async fn metrics(&self, req: MetricsRequest) -> Result<Vec<MetricSeries>, AzError> {
        let step = step_minutes(req.interval, req.start, req.end);
        let dims: Vec<(Option<String>, f64)> = match &req.split_by {
            Some(_) => vec![
                (Some("demo-host-0".into()), 1.2),
                (Some("demo-host-1".into()), 0.8),
            ],
            None => vec![(None, 1.0)],
        };
        let mut out = Vec::new();
        for name in &req.metric_names {
            for (dimension, scale) in &dims {
                let points = if req.interval == Interval::Full {
                    vec![(req.start, self.full(name, step).map(|v| v * scale))]
                } else {
                    let mut pts = Vec::new();
                    let mut at = req.start;
                    while at < req.end {
                        pts.push((at, self.bucket(name, at, req.end, step).map(|v| v * scale)));
                        at += Duration::minutes(step);
                    }
                    pts
                };
                out.push(MetricSeries {
                    name: name.clone(),
                    dimension: dimension.clone(),
                    points,
                });
            }
        }
        Ok(out)
    }

    async fn query(
        &self,
        _app_id: &str,
        kql_text: &str,
        _timespan: &str,
    ) -> Result<Table, AzError> {
        if kql_text == kql::summary_scalars() {
            return Ok(table(
                &[
                    "users_5m",
                    "users_range",
                    "sessions_range",
                    "servers_10m",
                    "response_p95_ms",
                ],
                vec![json!([165, 343, 360, 2, 480.0])],
            ));
        }
        if kql_text == kql::failure_tops() {
            return Ok(table(
                &["kind", "name", "detail", "n"],
                vec![
                    json!(["endpoint", "POST /api/orders", "", 12]),
                    json!(["endpoint", "GET /api/reports/export", "", 2]),
                    json!(["endpoint", "POST /api/auth/token", "", 1]),
                    json!([
                        "exception",
                        "SqlException",
                        "Execution Timeout Expired. The timeout period elapsed prior to completion of the operation.",
                        9
                    ]),
                    json!([
                        "exception",
                        "HttpRequestException",
                        "Response status code does not indicate success: 502 (Bad Gateway).",
                        3
                    ]),
                ],
            ));
        }
        if let Some(range) = TimeRange::ALL
            .into_iter()
            .find(|r| kql_text == kql::users_sessions_series(*r))
        {
            return Ok(self.users_sessions(range));
        }
        Ok(Table::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::alerts::{AlertConfig, evaluate};
    use crate::data::dashboard::{View, refresh_dashboard};
    use crate::data::store::Store;
    use crate::data::time::TimeRange;

    #[tokio::test]
    async fn demo_backend_trips_failure_alert() {
        let b = DemoBackend::new();
        let inst = demo_instances().remove(0);
        let now = chrono::Utc::now();
        let u = refresh_dashboard(&b, &inst, TimeRange::H4, View::Failures, now).await;
        assert!(u.errors.is_empty());
        let mut s = Store::default();
        s.apply(u, now);
        assert!(evaluate(&AlertConfig::default(), &s).failures());
        assert!(s.has_data(crate::data::catalog::SeriesId::Sessions));
        assert!(!s.has_data(crate::data::catalog::SeriesId::ProcessorCpu));
        assert_eq!(
            s.tops.as_ref().unwrap().value.endpoints[0].name,
            "POST /api/orders"
        );
    }

    #[tokio::test]
    async fn demo_instances_are_made_up() {
        let all = DemoBackend::new().instances().await.unwrap();
        assert_eq!(all.len(), 2);
        assert!(
            all.iter()
                .all(|i| i.subscription_id == "00000000-0000-0000-0000-000000000000")
        );
    }

    #[tokio::test]
    async fn demo_is_deterministic_for_the_same_window() {
        let b = DemoBackend::new();
        let inst = demo_instances().remove(0);
        let now = chrono::Utc::now();
        let a = refresh_dashboard(&b, &inst, TimeRange::H1, View::Overview, now).await;
        let c = refresh_dashboard(&b, &inst, TimeRange::H1, View::Overview, now).await;
        assert_eq!(
            a.series[&crate::data::catalog::SeriesId::RequestsCount],
            c.series[&crate::data::catalog::SeriesId::RequestsCount]
        );
    }
}
