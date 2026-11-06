//! The last good value of everything on screen, with timestamps for staleness.

use std::collections::HashMap;

use chrono::{DateTime, Duration, Utc};

use crate::azure::http::AzError;
use crate::data::catalog::SeriesId;
use crate::data::dashboard::{
    ALL_SOURCES, AlertWindow, DashboardUpdate, FailureTops, Scalars, Series, Source,
};

#[derive(Clone, Debug, PartialEq)]
pub struct Timed<T> {
    pub value: T,
    pub at: DateTime<Utc>,
}

impl<T> Timed<T> {
    pub fn stale_age(&self, now: DateTime<Utc>, refresh_secs: u64) -> Option<Duration> {
        let age = now - self.at;
        (age > Duration::seconds(2 * refresh_secs as i64 + 60)).then_some(age)
    }
}

#[derive(Clone, Debug, Default)]
pub struct Store {
    pub series: HashMap<SeriesId, Timed<Series>>,
    pub totals: HashMap<SeriesId, Timed<f64>>,
    pub scalars: Option<Timed<Scalars>>,
    pub alert_window: Option<Timed<AlertWindow>>,
    pub tops: Option<Timed<FailureTops>>,
    pub last_success: Option<DateTime<Utc>>,
    pub last_error: Option<(DateTime<Utc>, String)>,
    /// The current error of each source that failed on its last call.
    pub source_errors: HashMap<Source, String>,
}

fn describe(e: &AzError) -> String {
    e.to_string()
}

impl Store {
    /// Overwrites only what the update carries; anything a failed source
    /// would have provided keeps its previous value (shown as stale).
    pub fn apply(&mut self, u: DashboardUpdate, at: DateTime<Utc>) {
        for (source, e) in &u.errors {
            self.source_errors.insert(*source, describe(e));
        }
        for source in &u.succeeded {
            self.source_errors.remove(source);
            let (specs, totals) = source.covers();
            for spec in specs {
                if totals && !u.totals.contains_key(&spec.series) {
                    self.totals.remove(&spec.series);
                }
                if !totals && !u.series.contains_key(&spec.series) {
                    self.series.remove(&spec.series);
                }
            }
        }
        for (id, s) in u.series {
            self.series.insert(id, Timed { value: s, at });
        }
        for (id, v) in u.totals {
            self.totals.insert(id, Timed { value: v, at });
        }
        if let Some(s) = u.scalars {
            self.scalars = Some(Timed { value: s, at });
        }
        if let Some(w) = u.alert_window {
            self.alert_window = Some(Timed { value: w, at });
        }
        if let Some(t) = u.tops {
            self.tops = Some(Timed { value: t, at });
        }
        match u.errors.first() {
            None => self.last_success = Some(at),
            Some((_, e)) => self.last_error = Some((at, describe(e))),
        }
    }

    /// The current error of a source that provides `id`, if any.
    pub fn error_for(&self, id: SeriesId) -> Option<&str> {
        ALL_SOURCES
            .iter()
            .filter(|src| match id {
                SeriesId::Users | SeriesId::Sessions => **src == Source::UsersSessions,
                _ => src.covers().0.iter().any(|m| m.series == id),
            })
            .find_map(|src| self.source_errors.get(src).map(String::as_str))
    }

    /// The first current error, in source order, for the header.
    pub fn current_error(&self) -> Option<&str> {
        ALL_SOURCES
            .iter()
            .find_map(|src| self.source_errors.get(src).map(String::as_str))
    }

    pub fn clear(&mut self) {
        *self = Store::default();
    }

    pub fn has_data(&self, id: SeriesId) -> bool {
        self.series
            .get(&id)
            .is_some_and(|s| s.value.points.iter().any(|(_, v)| v.is_some()))
            || self.totals.contains_key(&id)
    }

    pub fn total(&self, id: SeriesId) -> Option<f64> {
        self.totals.get(&id).map(|t| t.value)
    }

    pub fn age(&self, id: SeriesId, now: DateTime<Utc>) -> Option<Duration> {
        self.series
            .get(&id)
            .map(|s| s.at)
            .or_else(|| self.totals.get(&id).map(|t| t.at))
            .map(|at| now - at)
    }

    pub fn is_stale(&self, id: SeriesId, now: DateTime<Utc>, refresh_secs: u64) -> bool {
        let limit = Duration::seconds(2 * refresh_secs as i64 + 60);
        self.age(id, now).is_some_and(|a| a > limit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::dashboard::{AlertWindow, DashboardUpdate, Scalars, Series, Source};
    use chrono::TimeZone;

    fn t0() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 4, 13, 0, 0).unwrap()
    }

    fn series(v: Option<f64>) -> Series {
        Series {
            points: vec![(t0(), v), (t0(), v)],
        }
    }

    fn full_update() -> DashboardUpdate {
        let mut u = DashboardUpdate::default();
        u.series.insert(SeriesId::RequestsFailed, series(Some(1.0)));
        u.totals.insert(SeriesId::RequestsFailed, 24.0);
        u.scalars = Some(Scalars {
            users_5m: Some(1.0),
            ..Default::default()
        });
        u.alert_window = Some(AlertWindow::default());
        u
    }

    #[test]
    fn store_keeps_old_values_and_marks_stale() {
        let mut s = Store::default();
        s.apply(full_update(), t0());
        let later = t0() + chrono::Duration::minutes(5);
        let mut partial = DashboardUpdate {
            scalars: Some(Scalars::default()),
            ..Default::default()
        };
        partial
            .errors
            .push((Source::CountMetrics, AzError::Network("down".into())));
        s.apply(partial, later);
        assert_eq!(s.series[&SeriesId::RequestsFailed].at, t0());
        assert!(s.is_stale(SeriesId::RequestsFailed, later, 30));
        assert!(!s.is_stale(
            SeriesId::RequestsFailed,
            t0() + chrono::Duration::seconds(100),
            30
        ));
        assert_eq!(s.last_success, Some(t0()));
        assert_eq!(s.last_error.as_ref().unwrap().0, later);
        assert_eq!(s.scalars.as_ref().unwrap().at, later);
        assert_eq!(s.total(SeriesId::RequestsFailed), Some(24.0));
        assert_eq!(
            s.age(SeriesId::RequestsFailed, later),
            Some(chrono::Duration::minutes(5))
        );
    }

    #[test]
    fn successful_source_drops_values_it_no_longer_reports() {
        use crate::data::alerts::{AlertConfig, AlertKind, evaluate};
        let mut s = Store::default();
        let mut u = DashboardUpdate::default();
        u.totals.insert(SeriesId::ServerResponse, 1500.0);
        u.series
            .insert(SeriesId::ServerResponse, series(Some(1500.0)));
        u.totals.insert(SeriesId::RequestsFailed, 3.0);
        u.succeeded = vec![
            Source::AverageTotals,
            Source::AverageMetrics,
            Source::CountTotals,
        ];
        s.apply(u, t0());
        assert!(evaluate(&AlertConfig::default(), &s, t0(), 30).is(AlertKind::Response));

        // Two hours later the averages succeed but carry no samples (app down),
        // and the count totals call fails.
        let mut later = DashboardUpdate {
            succeeded: vec![Source::AverageTotals, Source::AverageMetrics],
            ..Default::default()
        };
        later
            .errors
            .push((Source::CountTotals, AzError::Network("down".into())));
        let t2 = t0() + chrono::Duration::hours(2);
        s.apply(later, t2);
        assert_eq!(s.total(SeriesId::ServerResponse), None);
        assert!(!s.has_data(SeriesId::ServerResponse));
        assert!(!evaluate(&AlertConfig::default(), &s, t0(), 30).is(AlertKind::Response));
        assert_eq!(
            s.total(SeriesId::RequestsFailed),
            Some(3.0),
            "failed source keeps its value"
        );
    }

    #[test]
    fn source_errors_are_tracked_and_cleared() {
        let mut s = Store::default();
        let mut bad = DashboardUpdate::default();
        bad.errors.push((
            Source::AverageMetrics,
            AzError::Http {
                status: 403,
                message: "AuthorizationFailed: no access".into(),
            },
        ));
        s.apply(bad, t0());
        assert!(
            s.error_for(SeriesId::ServerResponse)
                .unwrap()
                .contains("403")
        );
        assert_eq!(s.error_for(SeriesId::RequestsFailed), None);
        let ok = DashboardUpdate {
            succeeded: vec![Source::AverageMetrics],
            ..Default::default()
        };
        s.apply(ok, t0());
        assert_eq!(s.error_for(SeriesId::ServerResponse), None);
        assert!(s.current_error().is_none());
    }

    #[test]
    fn empty_metric_is_no_data() {
        let mut s = Store::default();
        let mut u = DashboardUpdate::default();
        u.series.insert(SeriesId::PageLoadTotal, series(None));
        s.apply(u, t0());
        assert!(!s.has_data(SeriesId::PageLoadTotal));
        assert!(!s.has_data(SeriesId::Users));
        s.apply(full_update(), t0());
        assert!(s.has_data(SeriesId::RequestsFailed));
    }

    #[test]
    fn clear_forgets_everything() {
        let mut s = Store::default();
        s.apply(full_update(), t0());
        s.clear();
        assert!(!s.has_data(SeriesId::RequestsFailed));
        assert!(s.last_success.is_none());
    }
}
