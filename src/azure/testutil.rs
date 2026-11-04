//! Shared helpers for Azure client tests.

use std::sync::Arc;

use url::Url;
use wiremock::MockServer;

use crate::azure::auth::StaticTokenSource;
use crate::azure::http::{AzureHttp, Endpoints, RetryPolicy};

/// An `AzureHttp` whose ARM and App Insights endpoints both point at `server`.
pub fn http_for(server: &MockServer) -> AzureHttp {
    let base = Url::parse(&format!("{}/", server.uri())).expect("mock server URL");
    AzureHttp::new(
        Arc::new(StaticTokenSource("t".into())),
        Endpoints {
            arm: base.clone(),
            app_insights: base,
        },
    )
    .with_retry(RetryPolicy {
        max_attempts: 1,
        base_delay: std::time::Duration::from_millis(1),
        max_delay: std::time::Duration::from_millis(1),
    })
}

pub const DEMO_RESOURCE_ID: &str = "/subscriptions/00000000-0000-0000-0000-000000000000/resourceGroups/demo-rg/providers/microsoft.insights/components/demo-app-prod";
