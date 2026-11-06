//! Authenticated JSON requests to Azure with retry and backoff.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use reqwest::{Method, StatusCode};
use serde::Serialize;
use serde::de::DeserializeOwned;
use url::Url;

use crate::azure::auth::{Audience, AuthError, TokenSource};

#[derive(Clone, Debug)]
pub struct Endpoints {
    pub arm: Url,
    pub app_insights: Url,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            arm: Url::parse("https://management.azure.com/").expect("static URL"),
            app_insights: Url::parse("https://api.applicationinsights.io/").expect("static URL"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub base_delay: Duration,
    pub max_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(30),
        }
    }
}

impl RetryPolicy {
    fn backoff(&self, attempt: u32) -> Duration {
        let factor = 2u32.saturating_pow(attempt.saturating_sub(1));
        self.base_delay.saturating_mul(factor).min(self.max_delay)
    }
}

#[derive(Clone, Debug, thiserror::Error)]
pub enum AzError {
    #[error(transparent)]
    Auth(#[from] AuthError),
    #[error("throttled by Azure")]
    Throttled { retry_after: Option<Duration> },
    #[error("HTTP {status}: {message}")]
    Http { status: u16, message: String },
    #[error("network: {0}")]
    Network(String),
    #[error("unexpected response: {0}")]
    Decode(String),
}

#[derive(Clone)]
pub struct AzureHttp {
    client: reqwest::Client,
    tokens: Arc<dyn TokenSource>,
    endpoints: Endpoints,
    retry: RetryPolicy,
}

impl AzureHttp {
    pub fn new(tokens: Arc<dyn TokenSource>, endpoints: Endpoints) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .user_agent(concat!("appi-dashboard/", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            client,
            tokens,
            endpoints,
            retry: RetryPolicy::default(),
        }
    }

    pub fn with_retry(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }

    pub fn endpoints(&self) -> &Endpoints {
        &self.endpoints
    }

    pub async fn get_json<T: DeserializeOwned>(
        &self,
        aud: Audience,
        url: Url,
    ) -> Result<T, AzError> {
        self.send(aud, Method::GET, url, None).await
    }

    pub async fn post_json<B: Serialize + ?Sized, T: DeserializeOwned>(
        &self,
        aud: Audience,
        url: Url,
        body: &B,
    ) -> Result<T, AzError> {
        let body = serde_json::to_value(body).map_err(|e| AzError::Decode(e.to_string()))?;
        self.send(aud, Method::POST, url, Some(body)).await
    }

    async fn send<T: DeserializeOwned>(
        &self,
        aud: Audience,
        method: Method,
        url: Url,
        body: Option<serde_json::Value>,
    ) -> Result<T, AzError> {
        let max = self.retry.max_attempts.max(1);
        let mut invalidated = false;
        let mut attempt = 0;
        loop {
            attempt += 1;
            let last = attempt >= max;
            let token = self.tokens.token(aud).await?;
            let mut req = self
                .client
                .request(method.clone(), url.clone())
                .bearer_auth(token);
            if let Some(b) = &body {
                req = req.json(b);
            }
            let resp = match req.send().await {
                Ok(r) => r,
                Err(e) => {
                    if last {
                        return Err(AzError::Network(e.to_string()));
                    }
                    tokio::time::sleep(self.retry.backoff(attempt)).await;
                    continue;
                }
            };
            let status = resp.status();
            if status.is_success() {
                let bytes = resp
                    .bytes()
                    .await
                    .map_err(|e| AzError::Network(e.to_string()))?;
                return serde_json::from_slice(&bytes).map_err(|e| AzError::Decode(e.to_string()));
            }
            if status == StatusCode::TOO_MANY_REQUESTS {
                let retry_after = resp
                    .headers()
                    .get(reqwest::header::RETRY_AFTER)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| parse_retry_after(v, SystemTime::now()));
                // Long server-directed delays belong to the refresh scheduler;
                // capping them here would retry before Azure permits it.
                if last || retry_after.is_some_and(|wait| wait > self.retry.max_delay) {
                    return Err(AzError::Throttled { retry_after });
                }
                let wait = retry_after.unwrap_or_else(|| self.retry.backoff(attempt));
                tokio::time::sleep(wait).await;
                continue;
            }
            let text = resp.text().await.unwrap_or_default();
            if status == StatusCode::UNAUTHORIZED && !invalidated && !last {
                self.tokens.invalidate(aud);
                invalidated = true;
                continue;
            }
            if status.is_server_error() && !last {
                tokio::time::sleep(self.retry.backoff(attempt)).await;
                continue;
            }
            return Err(AzError::Http {
                status: status.as_u16(),
                message: azure_error_message(&text),
            });
        }
    }
}

/// Retry-After accepts either delta seconds or an HTTP date. Past dates
/// allow an immediate retry; malformed values use the policy's backoff.
fn parse_retry_after(value: &str, now: SystemTime) -> Option<Duration> {
    let value = value.trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    httpdate::parse_http_date(value)
        .ok()
        .map(|at| at.duration_since(now).unwrap_or_default())
}

/// Extracts `code: message` from an Azure error body, or the first 200
/// characters of any other body.
pub(crate) fn azure_error_message(body: &str) -> String {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(body) {
        let err = v.get("error");
        let code = err.and_then(|e| e.get("code")).and_then(|c| c.as_str());
        let message = err.and_then(|e| e.get("message")).and_then(|m| m.as_str());
        match (code, message) {
            (Some(c), Some(m)) => return format!("{c}: {m}"),
            (None, Some(m)) => return m.to_string(),
            (Some(c), None) => return c.to_string(),
            (None, None) => {}
        }
    }
    body.chars().take(200).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::azure::auth::{AuthError, StaticTokenSource};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn fast_retry() -> RetryPolicy {
        RetryPolicy {
            max_attempts: 3,
            base_delay: Duration::from_millis(10),
            max_delay: Duration::from_millis(50),
        }
    }

    fn client(tokens: Arc<dyn TokenSource>) -> AzureHttp {
        AzureHttp::new(tokens, Endpoints::default()).with_retry(fast_retry())
    }

    fn url(server: &MockServer, p: &str) -> Url {
        Url::parse(&format!("{}{}", server.uri(), p)).unwrap()
    }

    #[derive(serde::Deserialize, Debug)]
    struct Ok_ {
        ok: bool,
    }

    #[tokio::test]
    async fn sends_bearer_token() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/x"))
            .and(header("Authorization", "Bearer t"))
            .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"ok":true}"#))
            .mount(&server)
            .await;
        let r: Ok_ = client(Arc::new(StaticTokenSource("t".into())))
            .get_json(Audience::Arm, url(&server, "/x"))
            .await
            .unwrap();
        assert!(r.ok);
    }

    #[tokio::test]
    async fn posts_json_body() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/q"))
            .and(wiremock::matchers::body_json(
                serde_json::json!({"query": "x"}),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"ok":true}"#))
            .mount(&server)
            .await;
        let r: Ok_ = client(Arc::new(StaticTokenSource("t".into())))
            .post_json(
                Audience::AppInsights,
                url(&server, "/q"),
                &serde_json::json!({"query": "x"}),
            )
            .await
            .unwrap();
        assert!(r.ok);
    }

    #[tokio::test]
    async fn retries_429_honoring_retry_after() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "1"))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"ok":true}"#))
            .mount(&server)
            .await;
        let http = client(Arc::new(StaticTokenSource("t".into()))).with_retry(RetryPolicy {
            max_delay: Duration::from_secs(1),
            ..fast_retry()
        });
        let started = std::time::Instant::now();
        let r: Result<Ok_, _> = http.get_json(Audience::Arm, url(&server, "/x")).await;
        assert!(r.is_ok());
        assert!(started.elapsed() >= Duration::from_secs(1));
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn persistent_429_is_throttled() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "0"))
            .mount(&server)
            .await;
        let r: Result<Ok_, _> = client(Arc::new(StaticTokenSource("t".into())))
            .get_json(Audience::Arm, url(&server, "/x"))
            .await;
        assert!(matches!(r, Err(AzError::Throttled { .. })));
        assert_eq!(server.received_requests().await.unwrap().len(), 3);
    }

    #[tokio::test]
    async fn persistent_429_with_long_retry_after_returns_without_sleeping() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "60"))
            .expect(1)
            .mount(&server)
            .await;
        let http = AzureHttp::new(
            Arc::new(StaticTokenSource("t".into())),
            Endpoints::default(),
        );
        let r: Result<Ok_, _> = tokio::time::timeout(
            Duration::from_secs(1),
            http.get_json(Audience::Arm, url(&server, "/x")),
        )
        .await
        .expect("long Retry-After must return without sleeping");
        assert!(matches!(
            r,
            Err(AzError::Throttled { retry_after: Some(wait) }) if wait == Duration::from_secs(60)
        ));
    }

    #[tokio::test]
    async fn long_retry_after_http_date_is_throttled_without_retry() {
        let server = MockServer::start().await;
        let date = httpdate::fmt_http_date(SystemTime::now() + Duration::from_secs(120));
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", date))
            .expect(1)
            .mount(&server)
            .await;
        let http = AzureHttp::new(
            Arc::new(StaticTokenSource("t".into())),
            Endpoints::default(),
        );
        let r: Result<Ok_, _> = tokio::time::timeout(
            Duration::from_secs(1),
            http.get_json(Audience::Arm, url(&server, "/x")),
        )
        .await
        .expect("long HTTP-date Retry-After must return without sleeping");
        assert!(matches!(
            r,
            Err(AzError::Throttled { retry_after: Some(wait) })
                if wait > Duration::from_secs(110) && wait <= Duration::from_secs(120)
        ));
    }

    #[test]
    fn retry_after_parses_seconds_dates_and_invalid_values() {
        let now = httpdate::parse_http_date("Wed, 04 Nov 2026 13:00:00 GMT").unwrap();
        for value in [" 60 ", "Wed, 04 Nov 2026 13:01:00 GMT"] {
            assert_eq!(parse_retry_after(value, now), Some(Duration::from_secs(60)));
        }
        for value in ["0", "Wed, 04 Nov 2026 12:59:00 GMT"] {
            assert_eq!(parse_retry_after(value, now), Some(Duration::ZERO));
        }
        for value in ["", "-1", "tomorrow"] {
            assert_eq!(parse_retry_after(value, now), None);
        }
    }

    #[tokio::test]
    async fn gives_up_after_three_5xx() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;
        let r: Result<Ok_, _> = client(Arc::new(StaticTokenSource("t".into())))
            .get_json(Audience::Arm, url(&server, "/x"))
            .await;
        assert!(matches!(r, Err(AzError::Http { status: 503, .. })));
        assert_eq!(server.received_requests().await.unwrap().len(), 3);
    }

    #[tokio::test]
    async fn returns_4xx_without_retry() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(403).set_body_string(
                r#"{"error":{"code":"AuthorizationFailed","message":"no access"}}"#,
            ))
            .mount(&server)
            .await;
        let r: Result<Ok_, _> = client(Arc::new(StaticTokenSource("t".into())))
            .get_json(Audience::Arm, url(&server, "/x"))
            .await;
        match r {
            Err(AzError::Http { status, message }) => {
                assert_eq!(status, 403);
                assert_eq!(message, "AuthorizationFailed: no access");
            }
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    struct CountingTokens {
        invalidations: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl TokenSource for CountingTokens {
        async fn token(&self, _aud: Audience) -> Result<String, AuthError> {
            Ok("t".into())
        }
        fn invalidate(&self, _aud: Audience) {
            self.invalidations.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn invalidates_token_on_401() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(401))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"ok":true}"#))
            .mount(&server)
            .await;
        let tokens = Arc::new(CountingTokens {
            invalidations: AtomicUsize::new(0),
        });
        let r: Result<Ok_, _> = client(tokens.clone())
            .get_json(Audience::Arm, url(&server, "/x"))
            .await;
        assert!(r.is_ok());
        assert_eq!(tokens.invalidations.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn decode_error_is_reported() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
            .mount(&server)
            .await;
        let r: Result<Ok_, _> = client(Arc::new(StaticTokenSource("t".into())))
            .get_json(Audience::Arm, url(&server, "/x"))
            .await;
        assert!(matches!(r, Err(AzError::Decode(_))));
    }

    #[tokio::test]
    async fn auth_errors_pass_through() {
        struct NoLogin;
        #[async_trait::async_trait]
        impl TokenSource for NoLogin {
            async fn token(&self, _aud: Audience) -> Result<String, AuthError> {
                Err(AuthError::NotLoggedIn("run az login".into()))
            }
            fn invalidate(&self, _aud: Audience) {}
        }
        let r: Result<Ok_, _> = client(Arc::new(NoLogin))
            .get_json(Audience::Arm, Url::parse("http://127.0.0.1:9/x").unwrap())
            .await;
        assert!(matches!(r, Err(AzError::Auth(AuthError::NotLoggedIn(_)))));
    }

    #[test]
    fn error_message_extraction() {
        assert_eq!(
            azure_error_message(r#"{"error":{"code":"C","message":"M"}}"#),
            "C: M"
        );
        assert_eq!(azure_error_message("plain text"), "plain text");
        assert_eq!(azure_error_message(&"x".repeat(500)).chars().count(), 200);
    }

    #[test]
    fn default_endpoints() {
        let e = Endpoints::default();
        assert_eq!(e.arm.as_str(), "https://management.azure.com/");
        assert_eq!(
            e.app_insights.as_str(),
            "https://api.applicationinsights.io/"
        );
    }
}
