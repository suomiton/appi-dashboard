//! Azure Monitor Metrics REST client.

use chrono::{DateTime, Utc};
use serde::Deserialize;
use url::Url;

use crate::azure::auth::Audience;
use crate::azure::http::{AzError, AzureHttp};

pub const METRICS_API_VERSION: &str = "2023-10-01";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Aggregation {
    Average,
    Count,
    Total,
    Maximum,
    Minimum,
}

impl Aggregation {
    pub fn as_str(self) -> &'static str {
        match self {
            Aggregation::Average => "average",
            Aggregation::Count => "count",
            Aggregation::Total => "total",
            Aggregation::Maximum => "maximum",
            Aggregation::Minimum => "minimum",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interval {
    PT1M,
    PT5M,
    PT15M,
    Full,
}

impl Interval {
    pub fn as_str(self) -> &'static str {
        match self {
            Interval::PT1M => "PT1M",
            Interval::PT5M => "PT5M",
            Interval::PT15M => "PT15M",
            Interval::Full => "FULL",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MetricsRequest {
    pub resource_id: String,
    pub metric_names: Vec<String>,
    pub aggregation: Aggregation,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub interval: Interval,
    pub split_by: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MetricSeries {
    pub name: String,
    pub dimension: Option<String>,
    pub points: Vec<(DateTime<Utc>, Option<f64>)>,
}

fn iso(t: DateTime<Utc>) -> String {
    t.format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

pub fn metrics_url(arm: &Url, req: &MetricsRequest) -> Url {
    let rel = format!(
        "{}/providers/Microsoft.Insights/metrics",
        req.resource_id.trim_start_matches('/')
    );
    let mut url = arm.join(&rel).unwrap_or_else(|_| arm.clone());
    {
        let mut q = url.query_pairs_mut();
        q.append_pair("api-version", METRICS_API_VERSION)
            .append_pair("metricnames", &req.metric_names.join(","))
            .append_pair("aggregation", req.aggregation.as_str())
            .append_pair("timespan", &format!("{}/{}", iso(req.start), iso(req.end)))
            .append_pair("interval", req.interval.as_str());
        if let Some(dim) = &req.split_by {
            q.append_pair("$filter", &format!("{dim} eq '*'"));
        }
    }
    url
}

#[derive(Deserialize)]
struct Response {
    #[serde(default)]
    value: Vec<Metric>,
}

#[derive(Deserialize)]
struct Name {
    #[serde(default)]
    value: String,
}

#[derive(Deserialize)]
struct Metric {
    name: Name,
    #[serde(default)]
    timeseries: Vec<TimeSeries>,
}

#[derive(Deserialize)]
struct TimeSeries {
    #[serde(default)]
    metadatavalues: Vec<MetaValue>,
    #[serde(default)]
    data: Vec<serde_json::Map<String, serde_json::Value>>,
}

#[derive(Deserialize)]
struct MetaValue {
    #[serde(default)]
    value: Option<String>,
}

pub async fn fetch_metrics(
    http: &AzureHttp,
    req: &MetricsRequest,
) -> Result<Vec<MetricSeries>, AzError> {
    let url = metrics_url(&http.endpoints().arm, req);
    let resp: Response = http.get_json(Audience::Arm, url).await?;
    let field = req.aggregation.as_str();
    let mut out = Vec::new();
    for metric in resp.value {
        for ts in metric.timeseries {
            let dimension = ts.metadatavalues.into_iter().next().and_then(|m| m.value);
            let points = ts
                .data
                .iter()
                .filter_map(|p| {
                    let at = p.get("timeStamp")?.as_str()?;
                    let at = DateTime::parse_from_rfc3339(at).ok()?.with_timezone(&Utc);
                    Some((at, p.get(field).and_then(|v| v.as_f64())))
                })
                .collect();
            out.push(MetricSeries {
                name: metric.name.value.clone(),
                dimension,
                points,
            });
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::azure::testutil::{DEMO_RESOURCE_ID, http_for};
    use chrono::TimeZone;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn t(h: u32, m: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 4, h, m, 0).unwrap()
    }

    fn req(split: Option<&str>) -> MetricsRequest {
        MetricsRequest {
            resource_id: DEMO_RESOURCE_ID.into(),
            metric_names: vec!["requests/failed".into(), "requests/count".into()],
            aggregation: Aggregation::Count,
            start: t(10, 0),
            end: t(14, 0),
            interval: Interval::PT5M,
            split_by: split.map(str::to_string),
        }
    }

    #[test]
    fn metrics_url_has_expected_pairs() {
        let base = Url::parse("https://management.azure.com/").unwrap();
        let u = metrics_url(&base, &req(Some("cloud/roleInstance")));
        assert_eq!(
            u.path(),
            format!("{DEMO_RESOURCE_ID}/providers/Microsoft.Insights/metrics")
        );
        let pairs: Vec<(String, String)> = u.query_pairs().into_owned().collect();
        let has = |k: &str, v: &str| pairs.iter().any(|(a, b)| a == k && b == v);
        assert!(has("api-version", METRICS_API_VERSION));
        assert!(has("metricnames", "requests/failed,requests/count"));
        assert!(has("aggregation", "count"));
        assert!(has("interval", "PT5M"));
        assert!(has("timespan", "2026-11-04T10:00:00Z/2026-11-04T14:00:00Z"));
        assert!(has("$filter", "cloud/roleInstance eq '*'"));
    }

    #[test]
    fn metrics_url_without_split_has_no_filter() {
        let base = Url::parse("https://management.azure.com/").unwrap();
        let u = metrics_url(&base, &req(None));
        assert!(!u.query_pairs().any(|(k, _)| k == "$filter"));
    }

    #[test]
    fn enum_strings() {
        assert_eq!(Aggregation::Average.as_str(), "average");
        assert_eq!(Aggregation::Maximum.as_str(), "maximum");
        assert_eq!(Interval::PT15M.as_str(), "PT15M");
        assert_eq!(Interval::Full.as_str(), "FULL");
    }

    #[tokio::test]
    async fn metrics_parses_points_and_missing_values() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!(
                "{DEMO_RESOURCE_ID}/providers/Microsoft.Insights/metrics"
            )))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(include_str!("../../tests/fixtures/metrics_count.json")),
            )
            .mount(&server)
            .await;
        let series = fetch_metrics(&http_for(&server), &req(None)).await.unwrap();
        assert_eq!(
            series.len(),
            1,
            "metrics without timeseries yield no series"
        );
        assert_eq!(series[0].name, "requests/failed");
        assert_eq!(series[0].dimension, None);
        assert_eq!(
            series[0].points,
            vec![(t(10, 0), Some(3.0)), (t(10, 5), None)]
        );
    }

    #[tokio::test]
    async fn metrics_parses_split_dimension() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(include_str!("../../tests/fixtures/metrics_split.json")),
            )
            .mount(&server)
            .await;
        let mut r = req(Some("cloud/roleInstance"));
        r.aggregation = Aggregation::Average;
        let series = fetch_metrics(&http_for(&server), &r).await.unwrap();
        let dims: Vec<Option<&str>> = series.iter().map(|s| s.dimension.as_deref()).collect();
        assert_eq!(dims, [Some("demo-host-0"), Some("demo-host-1")]);
        assert_eq!(series[1].points, vec![(t(10, 0), Some(0.5))]);
    }
}
