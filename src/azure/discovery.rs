//! Finds every Application Insights resource the signed-in identity can see.

use serde::{Deserialize, Serialize};

use crate::azure::auth::Audience;
use crate::azure::http::{AzError, AzureHttp};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppInstance {
    pub name: String,
    pub resource_id: String,
    pub resource_group: String,
    pub subscription_id: String,
    pub subscription_name: String,
    pub tenant_id: String,
    pub app_id: String,
}

pub(crate) const DISCOVERY_QUERY: &str = "resources
| where type =~ 'microsoft.insights/components'
| project name, id, resourceGroup, subscriptionId, tenantId, appId = tostring(properties.AppId)
| join kind=leftouter (resourcecontainers | where type =~ 'microsoft.resources/subscriptions' | project subscriptionId, subscriptionName = name) on subscriptionId
| project name, id, resourceGroup, subscriptionId, subscriptionName, tenantId, appId
| order by name asc";

const RESOURCE_GRAPH_API_VERSION: &str = "2022-10-01";

#[derive(Deserialize)]
struct GraphPage {
    #[serde(default)]
    data: Vec<GraphRow>,
    #[serde(rename = "$skipToken", default)]
    skip_token: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphRow {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    resource_group: Option<String>,
    #[serde(default)]
    subscription_id: Option<String>,
    #[serde(default)]
    subscription_name: Option<String>,
    #[serde(default)]
    tenant_id: Option<String>,
    #[serde(default)]
    app_id: Option<String>,
}

impl GraphRow {
    fn into_instance(self) -> Option<AppInstance> {
        let app_id = self.app_id.filter(|a| !a.is_empty())?;
        Some(AppInstance {
            name: self.name?,
            resource_id: self.id?,
            resource_group: self.resource_group.unwrap_or_default(),
            subscription_id: self.subscription_id.unwrap_or_default(),
            subscription_name: self.subscription_name.unwrap_or_default(),
            tenant_id: self.tenant_id.unwrap_or_default(),
            app_id,
        })
    }
}

/// Lists App Insights instances across all subscriptions the token can see,
/// following Resource Graph paging. Rows without an app ID are skipped
/// because they cannot be queried.
pub async fn list_instances(http: &AzureHttp) -> Result<Vec<AppInstance>, AzError> {
    let mut url = http
        .endpoints()
        .arm
        .join("providers/Microsoft.ResourceGraph/resources")
        .map_err(|e| AzError::Decode(e.to_string()))?;
    url.query_pairs_mut()
        .append_pair("api-version", RESOURCE_GRAPH_API_VERSION);

    let mut out = Vec::new();
    let mut skip: Option<String> = None;
    // A safety cap: 50 pages of 1000 rows is far beyond any real tenant.
    for _ in 0..50 {
        let mut options = serde_json::json!({ "resultFormat": "objectArray", "$top": 1000 });
        if let Some(token) = &skip {
            options["$skipToken"] = serde_json::Value::String(token.clone());
        }
        let body = serde_json::json!({ "query": DISCOVERY_QUERY, "options": options });
        let page: GraphPage = http.post_json(Audience::Arm, url.clone(), &body).await?;
        out.extend(page.data.into_iter().filter_map(GraphRow::into_instance));
        match page.skip_token.filter(|t| !t.is_empty()) {
            Some(t) => skip = Some(t),
            None => break,
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::azure::testutil::http_for;
    use wiremock::matchers::{body_string_contains, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn discovery_follows_skip_token_and_skips_rows_without_app_id() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/providers/Microsoft.ResourceGraph/resources"))
            .and(body_string_contains("\"$skipToken\":\"abc\""))
            .respond_with(ResponseTemplate::new(200).set_body_string(include_str!(
                "../../tests/fixtures/resource_graph_page2.json"
            )))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/providers/Microsoft.ResourceGraph/resources"))
            .respond_with(ResponseTemplate::new(200).set_body_string(include_str!(
                "../../tests/fixtures/resource_graph_page1.json"
            )))
            .mount(&server)
            .await;

        let all = list_instances(&http_for(&server)).await.unwrap();
        let names: Vec<&str> = all.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(
            names,
            ["demo-app-prod", "demo-app-staging", "demo-func-prod"]
        );
        assert_eq!(all[0].subscription_name, "Demo Subscription");
        assert_eq!(all[0].app_id, "11111111-1111-1111-1111-111111111111");
        assert_eq!(all[2].subscription_name, "");

        let reqs = server.received_requests().await.unwrap();
        assert_eq!(reqs.len(), 2);
        let first = String::from_utf8_lossy(&reqs[0].body).to_string();
        assert!(first.contains("microsoft.insights/components"));
        assert!(!first.contains("skipToken"));
        assert!(
            reqs[0]
                .url
                .query()
                .unwrap_or("")
                .contains("api-version=2022-10-01")
        );
    }
}
