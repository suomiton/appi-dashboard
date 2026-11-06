//! Application Insights query (KQL) client.

use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::azure::auth::Audience;
use crate::azure::http::{AzError, AzureHttp};

/// The first table of a query result, with forgiving typed accessors.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Table {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<serde_json::Value>>,
}

impl Table {
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    fn cell(&self, row: usize, col: &str) -> Option<&serde_json::Value> {
        let idx = self.columns.iter().position(|c| c == col)?;
        self.rows.get(row)?.get(idx).filter(|v| !v.is_null())
    }

    pub fn str(&self, row: usize, col: &str) -> Option<&str> {
        self.cell(row, col)?.as_str()
    }

    /// A JSON number, or a string that parses as one.
    pub fn f64(&self, row: usize, col: &str) -> Option<f64> {
        match self.cell(row, col)? {
            serde_json::Value::Number(n) => n.as_f64(),
            serde_json::Value::String(s) => s.trim().parse::<f64>().ok(),
            _ => None,
        }
    }

    pub fn time(&self, row: usize, col: &str) -> Option<DateTime<Utc>> {
        let s = self.str(row, col)?;
        DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|t| t.with_timezone(&Utc))
    }

    pub fn json(&self, row: usize, col: &str) -> Option<&serde_json::Value> {
        self.cell(row, col)
    }
}

/// Builds a table from column names and JSON-array rows for demo data and fixtures.
pub(crate) fn table(columns: &[&str], rows: Vec<serde_json::Value>) -> Table {
    Table {
        columns: columns.iter().map(|c| c.to_string()).collect(),
        rows: rows
            .into_iter()
            .map(|r| r.as_array().cloned().unwrap_or_default())
            .collect(),
    }
}

#[derive(Deserialize)]
struct Response {
    #[serde(default)]
    tables: Vec<RawTable>,
}

#[derive(Deserialize)]
struct RawTable {
    #[serde(default)]
    columns: Vec<RawColumn>,
    #[serde(default)]
    rows: Vec<Vec<serde_json::Value>>,
}

#[derive(Deserialize)]
struct RawColumn {
    name: String,
}

/// Runs a KQL query against one App Insights app. `timespan` is an ISO 8601
/// duration such as `PT4H`.
pub async fn query(
    http: &AzureHttp,
    app_id: &str,
    kql: &str,
    timespan: &str,
) -> Result<Table, AzError> {
    let url = http
        .endpoints()
        .app_insights
        .join(&format!("v1/apps/{app_id}/query"))
        .map_err(|e| AzError::Decode(e.to_string()))?;
    let body = serde_json::json!({ "query": kql, "timespan": timespan });
    let resp: Response = http.post_json(Audience::AppInsights, url, &body).await?;
    Ok(resp
        .tables
        .into_iter()
        .next()
        .map(|t| Table {
            columns: t.columns.into_iter().map(|c| c.name).collect(),
            rows: t.rows,
        })
        .unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::azure::testutil::http_for;
    use chrono::TimeZone;
    use wiremock::matchers::{body_partial_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn fixture_table() -> Table {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/apps/app-1/query"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(include_str!("../../tests/fixtures/logs_table.json")),
            )
            .mount(&server)
            .await;
        query(&http_for(&server), "app-1", "requests | take 2", "PT4H")
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn logs_parses_table() {
        let t = fixture_table().await;
        assert_eq!(t.len(), 2);
        assert!(!t.is_empty());
        assert_eq!(t.columns, ["timestamp", "name", "duration", "n", "details"]);
        assert_eq!(t.str(0, "name"), Some("POST /api/orders"));
        assert_eq!(t.f64(0, "duration"), Some(3.38), "numeric strings parse");
        assert_eq!(t.f64(1, "duration"), Some(30010.5));
        assert_eq!(t.f64(0, "n"), Some(12.0));
        assert_eq!(t.f64(1, "n"), None, "null is None");
        assert_eq!(
            t.time(1, "timestamp"),
            Some(Utc.with_ymd_and_hms(2026, 11, 4, 13, 57, 40).unwrap())
        );
        assert!(t.time(0, "timestamp").is_some(), "fractional seconds parse");
        assert!(t.json(0, "details").is_some_and(|v| v.is_array()));
        assert_eq!(t.str(0, "missing"), None);
        assert_eq!(t.str(9, "name"), None);
        assert_eq!(t.f64(0, "name"), None);
    }

    #[tokio::test]
    async fn logs_sends_query_and_timespan() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/apps/app-1/query"))
            .and(body_partial_json(
                serde_json::json!({"query": "requests | take 1", "timespan": "PT4H"}),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"tables":[]}"#))
            .mount(&server)
            .await;
        let t = query(&http_for(&server), "app-1", "requests | take 1", "PT4H")
            .await
            .unwrap();
        assert!(t.is_empty());
    }
}
