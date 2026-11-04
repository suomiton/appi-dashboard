//! Where dashboard data comes from: Azure, the demo generator, or test fakes.

use crate::azure::discovery::{AppInstance, list_instances};
use crate::azure::http::{AzError, AzureHttp};
use crate::azure::logs::{Table, query};
use crate::azure::metrics::{MetricSeries, MetricsRequest, fetch_metrics};

#[async_trait::async_trait]
pub trait Backend: Send + Sync {
    async fn instances(&self) -> Result<Vec<AppInstance>, AzError>;
    async fn metrics(&self, req: MetricsRequest) -> Result<Vec<MetricSeries>, AzError>;
    async fn query(&self, app_id: &str, kql: &str, timespan: &str) -> Result<Table, AzError>;
}

/// The real thing: Resource Graph, Azure Monitor Metrics and App Insights.
pub struct AzureBackend {
    http: AzureHttp,
}

impl AzureBackend {
    pub fn new(http: AzureHttp) -> Self {
        Self { http }
    }
}

#[async_trait::async_trait]
impl Backend for AzureBackend {
    async fn instances(&self) -> Result<Vec<AppInstance>, AzError> {
        list_instances(&self.http).await
    }

    async fn metrics(&self, req: MetricsRequest) -> Result<Vec<MetricSeries>, AzError> {
        fetch_metrics(&self.http, &req).await
    }

    async fn query(&self, app_id: &str, kql: &str, timespan: &str) -> Result<Table, AzError> {
        query(&self.http, app_id, kql, timespan).await
    }
}

/// Test double: canned metric responses from a closure, canned query results
/// keyed by exact KQL text, and a record of every call.
#[cfg(test)]
pub mod fake {
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::time::Duration;

    use super::Backend;
    use crate::azure::discovery::AppInstance;
    use crate::azure::http::AzError;
    use crate::azure::logs::Table;
    use crate::azure::metrics::{MetricSeries, MetricsRequest};

    type MetricFn =
        Box<dyn Fn(&MetricsRequest) -> Result<Vec<MetricSeries>, AzError> + Send + Sync>;

    pub struct FakeBackend {
        pub instances: Result<Vec<AppInstance>, AzError>,
        metric_fn: MetricFn,
        queries: HashMap<String, Result<Table, AzError>>,
        query_delay: Option<Duration>,
        pub calls: Mutex<Vec<String>>,
    }

    impl Default for FakeBackend {
        fn default() -> Self {
            Self::new()
        }
    }

    impl FakeBackend {
        pub fn new() -> Self {
            Self {
                instances: Ok(Vec::new()),
                metric_fn: Box::new(|_| Ok(Vec::new())),
                queries: HashMap::new(),
                query_delay: None,
                calls: Mutex::new(Vec::new()),
            }
        }

        pub fn with_instances(mut self, v: Vec<AppInstance>) -> Self {
            self.instances = Ok(v);
            self
        }

        pub fn with_metrics(
            mut self,
            f: impl Fn(&MetricsRequest) -> Result<Vec<MetricSeries>, AzError> + Send + Sync + 'static,
        ) -> Self {
            self.metric_fn = Box::new(f);
            self
        }

        pub fn with_query(
            mut self,
            kql: impl Into<String>,
            result: Result<Table, AzError>,
        ) -> Self {
            self.queries.insert(kql.into(), result);
            self
        }

        pub fn with_query_delay(mut self, d: Duration) -> Self {
            self.query_delay = Some(d);
            self
        }

        pub fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl Backend for FakeBackend {
        async fn instances(&self) -> Result<Vec<AppInstance>, AzError> {
            self.calls.lock().unwrap().push("instances".into());
            self.instances.clone()
        }

        async fn metrics(&self, req: MetricsRequest) -> Result<Vec<MetricSeries>, AzError> {
            self.calls.lock().unwrap().push(format!(
                "metrics:{}:{}:{}",
                req.aggregation.as_str(),
                req.interval.as_str(),
                req.metric_names.join(",")
            ));
            (self.metric_fn)(&req)
        }

        async fn query(&self, _app_id: &str, kql: &str, timespan: &str) -> Result<Table, AzError> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("query:{timespan}:{kql}"));
            if let Some(d) = self.query_delay {
                tokio::time::sleep(d).await;
            }
            self.queries
                .get(kql)
                .cloned()
                .unwrap_or(Ok(Table::default()))
        }
    }

    /// Builds a `Table` from column names and JSON rows.
    pub fn table(columns: &[&str], rows: Vec<serde_json::Value>) -> Table {
        Table {
            columns: columns.iter().map(|c| c.to_string()).collect(),
            rows: rows
                .into_iter()
                .map(|r| r.as_array().cloned().unwrap_or_default())
                .collect(),
        }
    }
}
