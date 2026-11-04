//! Which Azure Monitor metrics feed which chart series.
//!
//! Metric names are exactly as in Microsoft's supported-metrics list for
//! `microsoft.insights/components`. Users and sessions are log-based metrics
//! in the portal, so they come from KQL instead (see `kql.rs`).

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SeriesId {
    RequestsCount,
    RequestsFailed,
    ExceptionsServer,
    ExceptionsBrowser,
    DependenciesFailed,
    AvailabilityCount,
    ServerResponse,
    PageLoadTotal,
    PageLoadNetwork,
    PageLoadProcessing,
    PageLoadSend,
    PageLoadReceive,
    ProcessCpu,
    ProcessorCpu,
    MemoryAvailable,
    ProcessIo,
    Availability,
    Users,
    Sessions,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MetricSpec {
    pub series: SeriesId,
    pub metric: &'static str,
}

const fn spec(series: SeriesId, metric: &'static str) -> MetricSpec {
    MetricSpec { series, metric }
}

/// Fetched with `Aggregation::Count`.
pub const COUNT_METRICS: &[MetricSpec] = &[
    spec(SeriesId::RequestsCount, "requests/count"),
    spec(SeriesId::RequestsFailed, "requests/failed"),
    spec(SeriesId::ExceptionsServer, "exceptions/server"),
    spec(SeriesId::ExceptionsBrowser, "exceptions/browser"),
    spec(SeriesId::DependenciesFailed, "dependencies/failed"),
    spec(SeriesId::AvailabilityCount, "availabilityResults/count"),
];

/// Fetched with `Aggregation::Average`.
pub const AVERAGE_METRICS: &[MetricSpec] = &[
    spec(SeriesId::ServerResponse, "requests/duration"),
    spec(SeriesId::PageLoadTotal, "browserTimings/totalDuration"),
    spec(SeriesId::PageLoadNetwork, "browserTimings/networkDuration"),
    spec(
        SeriesId::PageLoadProcessing,
        "browserTimings/processingDuration",
    ),
    spec(SeriesId::PageLoadSend, "browserTimings/sendDuration"),
    spec(SeriesId::PageLoadReceive, "browserTimings/receiveDuration"),
    spec(
        SeriesId::ProcessCpu,
        "performanceCounters/processCpuPercentage",
    ),
    spec(
        SeriesId::ProcessorCpu,
        "performanceCounters/processorCpuPercentage",
    ),
    spec(
        SeriesId::MemoryAvailable,
        "performanceCounters/memoryAvailableBytes",
    ),
    spec(
        SeriesId::ProcessIo,
        "performanceCounters/processIOBytesPerSecond",
    ),
    spec(
        SeriesId::Availability,
        "availabilityResults/availabilityPercentage",
    ),
];

/// Count, PT1M, last 10 minutes: feeds the 5-minute alert window.
pub const ALERT_METRICS: &[MetricSpec] = &[
    spec(SeriesId::RequestsFailed, "requests/failed"),
    spec(SeriesId::ExceptionsServer, "exceptions/server"),
    spec(SeriesId::DependenciesFailed, "dependencies/failed"),
];

pub fn series_for(metric: &str) -> Option<SeriesId> {
    COUNT_METRICS
        .iter()
        .chain(AVERAGE_METRICS)
        .find(|m| m.metric.eq_ignore_ascii_case(metric))
        .map(|m| m.series)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn groups_are_unique_and_within_api_limit() {
        for group in [COUNT_METRICS, AVERAGE_METRICS, ALERT_METRICS] {
            let names: HashSet<&str> = group.iter().map(|m| m.metric).collect();
            assert_eq!(names.len(), group.len());
            assert!(group.len() <= 20);
        }
        assert_eq!(COUNT_METRICS.len(), 6);
        assert_eq!(AVERAGE_METRICS.len(), 11);
        assert_eq!(ALERT_METRICS.len(), 3);
    }

    #[test]
    fn lookup_by_metric_name() {
        assert_eq!(
            series_for("requests/failed"),
            Some(SeriesId::RequestsFailed)
        );
        assert_eq!(
            series_for("performanceCounters/processIOBytesPerSecond"),
            Some(SeriesId::ProcessIo)
        );
        assert_eq!(
            series_for("browserTimings/receiveDuration"),
            Some(SeriesId::PageLoadReceive)
        );
        assert_eq!(series_for("nope"), None);
    }
}
