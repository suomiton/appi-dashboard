//! Threshold rules that turn panels red.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::data::catalog::SeriesId;
use crate::data::store::Store;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AlertConfig {
    pub failed_requests_5m: f64,
    pub server_exceptions_5m: f64,
    pub dependency_failures_5m: f64,
    pub availability_below_pct: f64,
    pub response_avg_above_ms: f64,
}

impl Default for AlertConfig {
    fn default() -> Self {
        Self {
            failed_requests_5m: 5.0,
            server_exceptions_5m: 10.0,
            dependency_failures_5m: 10.0,
            availability_below_pct: 99.0,
            response_avg_above_ms: 1000.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AlertKind {
    Failures,
    ExceptionsDeps,
    Availability,
    Response,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AlertState {
    pub active: HashSet<AlertKind>,
}

impl AlertState {
    pub fn failures(&self) -> bool {
        self.is(AlertKind::Failures)
    }

    pub fn is(&self, k: AlertKind) -> bool {
        self.active.contains(&k)
    }
}

pub fn evaluate(cfg: &AlertConfig, store: &Store) -> AlertState {
    let mut active = HashSet::new();
    if let Some(w) = &store.alert_window {
        let w = &w.value;
        if w.failed_5m >= cfg.failed_requests_5m {
            active.insert(AlertKind::Failures);
        }
        if w.server_exceptions_5m >= cfg.server_exceptions_5m
            || w.dependency_failures_5m >= cfg.dependency_failures_5m
        {
            active.insert(AlertKind::ExceptionsDeps);
        }
    }
    if store
        .total(SeriesId::Availability)
        .is_some_and(|a| a < cfg.availability_below_pct)
    {
        active.insert(AlertKind::Availability);
    }
    if store
        .total(SeriesId::ServerResponse)
        .is_some_and(|r| r > cfg.response_avg_above_ms)
    {
        active.insert(AlertKind::Response);
    }
    AlertState { active }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::dashboard::{AlertWindow, DashboardUpdate};
    use chrono::{TimeZone, Utc};

    fn store_with(failed_5m: f64, availability: Option<f64>) -> Store {
        let mut u = DashboardUpdate {
            alert_window: Some(AlertWindow {
                failed_5m,
                ..Default::default()
            }),
            ..Default::default()
        };
        if let Some(a) = availability {
            u.totals.insert(SeriesId::Availability, a);
        }
        let mut s = Store::default();
        s.apply(u, Utc.with_ymd_and_hms(2026, 11, 4, 13, 0, 0).unwrap());
        s
    }

    #[test]
    fn failure_threshold() {
        let cfg = AlertConfig::default();
        assert!(evaluate(&cfg, &store_with(5.0, None)).failures());
        assert!(!evaluate(&cfg, &store_with(4.0, None)).failures());
    }

    #[test]
    fn availability_threshold() {
        let cfg = AlertConfig::default();
        assert!(evaluate(&cfg, &store_with(0.0, Some(98.5))).is(AlertKind::Availability));
        assert!(!evaluate(&cfg, &store_with(0.0, Some(100.0))).is(AlertKind::Availability));
        assert!(!evaluate(&cfg, &store_with(0.0, None)).is(AlertKind::Availability));
    }

    #[test]
    fn defaults() {
        let c = AlertConfig::default();
        assert_eq!(
            (
                c.failed_requests_5m,
                c.server_exceptions_5m,
                c.dependency_failures_5m
            ),
            (5.0, 10.0, 10.0)
        );
        assert_eq!(
            (c.availability_below_pct, c.response_avg_above_ms),
            (99.0, 1000.0)
        );
        assert!(!evaluate(&c, &Store::default()).failures());
    }
}
