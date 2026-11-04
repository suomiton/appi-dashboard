//! Access tokens from the signed-in Azure CLI (`az account get-access-token`).

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use chrono::{DateTime, Local, NaiveDateTime, TimeZone, Utc};

/// The Azure APIs this app needs tokens for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Audience {
    Arm,
    AppInsights,
}

impl Audience {
    pub fn resource(self) -> &'static str {
        match self {
            Audience::Arm => "https://management.azure.com",
            Audience::AppInsights => "https://api.applicationinsights.io",
        }
    }

    const ALL: [Audience; 2] = [Audience::Arm, Audience::AppInsights];
}

#[derive(Clone, Debug, thiserror::Error)]
pub enum AuthError {
    #[error("Azure CLI (az) not found")]
    AzNotInstalled,
    #[error("not signed in: {0}")]
    NotLoggedIn(String),
    #[error("az failed: {0}")]
    Other(String),
}

#[async_trait::async_trait]
pub trait TokenSource: Send + Sync {
    async fn token(&self, aud: Audience) -> Result<String, AuthError>;
    fn invalidate(&self, aud: Audience);
}

/// A token source that always returns the same string. Used by tests and the
/// demo mode, where no real Azure calls are made.
pub struct StaticTokenSource(pub String);

#[async_trait::async_trait]
impl TokenSource for StaticTokenSource {
    async fn token(&self, _aud: Audience) -> Result<String, AuthError> {
        Ok(self.0.clone())
    }
    fn invalidate(&self, _aud: Audience) {}
}

#[derive(Clone, Debug)]
pub(crate) struct CachedToken {
    pub access_token: String,
    pub expires_at: DateTime<Utc>,
}

const REFRESH_MARGIN_MINUTES: i64 = 5;
const AZ_TIMEOUT: Duration = Duration::from_secs(60);

/// Gets tokens by running the Azure CLI, caching them until five minutes
/// before expiry. Concurrent callers for the same audience share one `az` run.
pub struct AzCliTokenSource {
    program: String,
    cache: StdMutex<HashMap<Audience, CachedToken>>,
    fetch_locks: HashMap<Audience, tokio::sync::Mutex<()>>,
}

impl Default for AzCliTokenSource {
    fn default() -> Self {
        Self::new()
    }
}

impl AzCliTokenSource {
    pub fn new() -> Self {
        Self::with_program(if cfg!(windows) { "az.cmd" } else { "az" })
    }

    pub fn with_program(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            cache: StdMutex::new(HashMap::new()),
            fetch_locks: Audience::ALL
                .iter()
                .map(|a| (*a, tokio::sync::Mutex::new(())))
                .collect(),
        }
    }

    fn cached(&self, aud: Audience) -> Option<String> {
        let cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        cache.get(&aud).and_then(|t| {
            let fresh_until = t.expires_at - chrono::Duration::minutes(REFRESH_MARGIN_MINUTES);
            (Utc::now() < fresh_until).then(|| t.access_token.clone())
        })
    }

    async fn run_az(&self, aud: Audience) -> Result<CachedToken, AuthError> {
        let mut cmd = tokio::process::Command::new(&self.program);
        cmd.args([
            "account",
            "get-access-token",
            "--resource",
            aud.resource(),
            "--output",
            "json",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
        let output = match tokio::time::timeout(AZ_TIMEOUT, cmd.output()).await {
            Err(_) => return Err(AuthError::Other("az timed out".into())),
            Ok(Err(e)) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(AuthError::AzNotInstalled);
            }
            Ok(Err(e)) => return Err(AuthError::Other(e.to_string())),
            Ok(Ok(o)) => o,
        };
        if !output.status.success() {
            return Err(classify_az_failure(&String::from_utf8_lossy(
                &output.stderr,
            )));
        }
        parse_az_token(&String::from_utf8_lossy(&output.stdout), Utc::now())
    }
}

#[async_trait::async_trait]
impl TokenSource for AzCliTokenSource {
    async fn token(&self, aud: Audience) -> Result<String, AuthError> {
        if let Some(t) = self.cached(aud) {
            return Ok(t);
        }
        let lock = self
            .fetch_locks
            .get(&aud)
            .ok_or_else(|| AuthError::Other("unknown audience".into()))?;
        let _guard = lock.lock().await;
        // Another caller may have fetched while we waited.
        if let Some(t) = self.cached(aud) {
            return Ok(t);
        }
        let fresh = self.run_az(aud).await?;
        let token = fresh.access_token.clone();
        self.cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(aud, fresh);
        Ok(token)
    }

    fn invalidate(&self, aud: Audience) {
        self.cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&aud);
    }
}

/// Parses `az account get-access-token --output json`.
pub(crate) fn parse_az_token(json: &str, now: DateTime<Utc>) -> Result<CachedToken, AuthError> {
    let v: serde_json::Value = serde_json::from_str(json)
        .map_err(|e| AuthError::Other(format!("unreadable az output: {e}")))?;
    let access_token = v
        .get("accessToken")
        .and_then(|t| t.as_str())
        .filter(|t| !t.is_empty())
        .ok_or_else(|| AuthError::Other("no accessToken in az output".into()))?
        .to_string();

    let epoch = v.get("expires_on").and_then(|e| match e {
        serde_json::Value::Number(n) => n.as_i64(),
        serde_json::Value::String(s) => s.trim().parse::<i64>().ok(),
        _ => None,
    });
    let expires_at = epoch
        .and_then(|secs| DateTime::<Utc>::from_timestamp(secs, 0))
        .or_else(|| {
            let s = v.get("expiresOn")?.as_str()?;
            let naive = NaiveDateTime::parse_from_str(s.trim(), "%Y-%m-%d %H:%M:%S%.f").ok()?;
            Local
                .from_local_datetime(&naive)
                .earliest()
                .map(|t| t.with_timezone(&Utc))
        })
        .unwrap_or(now + chrono::Duration::minutes(REFRESH_MARGIN_MINUTES));

    Ok(CachedToken {
        access_token,
        expires_at,
    })
}

/// Turns `az` stderr into an error the UI can explain.
pub(crate) fn classify_az_failure(stderr: &str) -> AuthError {
    let line = stderr
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("az exited with an error")
        .to_string();
    let lower = stderr.to_lowercase();
    if lower.contains("az login") || lower.contains("aadsts") || lower.contains("expired") {
        AuthError::NotLoggedIn(line)
    } else {
        AuthError::Other(line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Local, NaiveDate, TimeZone, Utc};

    fn now() -> DateTime<Utc> {
        Utc::now()
    }

    #[test]
    fn parses_epoch_expiry() {
        let t = parse_az_token(
            r#"{"accessToken":"tok","expires_on":1793800000,"expiresOn":"2026-11-04 16:00:00.000000"}"#,
            now(),
        )
        .unwrap();
        assert_eq!(t.access_token, "tok");
        assert_eq!(t.expires_at.timestamp(), 1793800000);
    }

    #[test]
    fn parses_epoch_expiry_given_as_string() {
        let t =
            parse_az_token(r#"{"accessToken":"tok","expires_on":"1793800000"}"#, now()).unwrap();
        assert_eq!(t.expires_at.timestamp(), 1793800000);
    }

    #[test]
    fn falls_back_to_local_expires_on() {
        let t = parse_az_token(
            r#"{"accessToken":"tok","expiresOn":"2026-11-04 16:00:00.000000"}"#,
            now(),
        )
        .unwrap();
        let naive = NaiveDate::from_ymd_opt(2026, 11, 4)
            .unwrap()
            .and_hms_opt(16, 0, 0)
            .unwrap();
        let expected = Local
            .from_local_datetime(&naive)
            .single()
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(t.expires_at, expected);
    }

    #[test]
    fn missing_expiry_means_five_minutes() {
        let n = now();
        let t = parse_az_token(r#"{"accessToken":"t"}"#, n).unwrap();
        assert_eq!(t.expires_at, n + chrono::Duration::minutes(5));
    }

    #[test]
    fn missing_access_token_is_an_error() {
        assert!(matches!(
            parse_az_token(r#"{"expires_on":1}"#, now()),
            Err(AuthError::Other(_))
        ));
    }

    #[test]
    fn classifies_not_logged_in() {
        assert!(matches!(
            classify_az_failure("ERROR: Please run 'az login' to setup account."),
            AuthError::NotLoggedIn(_)
        ));
        assert!(matches!(
            classify_az_failure("\nERROR: AADSTS700082: The refresh token has expired"),
            AuthError::NotLoggedIn(_)
        ));
        assert!(matches!(
            classify_az_failure("ERROR: something else"),
            AuthError::Other(_)
        ));
    }

    #[test]
    fn audience_resources() {
        assert_eq!(Audience::Arm.resource(), "https://management.azure.com");
        assert_eq!(
            Audience::AppInsights.resource(),
            "https://api.applicationinsights.io"
        );
    }

    #[tokio::test]
    async fn missing_program_is_not_installed() {
        let s = AzCliTokenSource::with_program("definitely-not-az-xyz");
        assert!(matches!(
            s.token(Audience::Arm).await,
            Err(AuthError::AzNotInstalled)
        ));
    }

    #[tokio::test]
    async fn static_token_source_returns_its_token() {
        let s = StaticTokenSource("abc".into());
        assert_eq!(s.token(Audience::AppInsights).await.unwrap(), "abc");
    }

    /// Writes a fake `az` script that sleeps, counts its invocations and
    /// prints a token expiring `expires_in` seconds from now.
    ///
    /// The file is written by a short-lived `sh` child rather than by this
    /// process: if this process held the write handle while another test
    /// thread forks, exec of the script fails with ETXTBSY ("Text file busy").
    #[cfg(unix)]
    fn fake_az(dir: &std::path::Path, expires_in: i64) -> (String, std::path::PathBuf) {
        let counter = dir.join("count");
        let script = dir.join("fake-az");
        let body = format!(
            "#!/bin/sh\nsleep 1\necho x >> '{}'\nnow=$(date +%s)\necho \"{{\\\"accessToken\\\":\\\"tok\\\",\\\"expires_on\\\":$((now+{}))}}\"\n",
            counter.display(),
            expires_in
        );
        let status = std::process::Command::new("sh")
            .arg("-c")
            .arg(r#"printf '%s' "$1" > "$2" && chmod 755 "$2""#)
            .arg("sh")
            .arg(&body)
            .arg(&script)
            .status()
            .unwrap();
        assert!(status.success());
        (script.display().to_string(), counter)
    }

    #[cfg(unix)]
    fn count_lines(p: &std::path::Path) -> usize {
        std::fs::read_to_string(p)
            .map(|s| s.lines().count())
            .unwrap_or(0)
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_callers_share_one_az_run() {
        let dir = tempfile::tempdir().unwrap();
        let (program, counter) = fake_az(dir.path(), 3600);
        let src = std::sync::Arc::new(AzCliTokenSource::with_program(program));
        let mut handles = Vec::new();
        for _ in 0..5 {
            let s = src.clone();
            handles.push(tokio::spawn(async move { s.token(Audience::Arm).await }));
        }
        for h in handles {
            assert_eq!(h.await.unwrap().unwrap(), "tok");
        }
        assert_eq!(count_lines(&counter), 1);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn refreshes_within_five_minutes_of_expiry() {
        let dir = tempfile::tempdir().unwrap();
        let (program, counter) = fake_az(dir.path(), 240);
        let src = AzCliTokenSource::with_program(program);
        src.token(Audience::Arm).await.unwrap();
        src.token(Audience::Arm).await.unwrap();
        assert_eq!(count_lines(&counter), 2);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn invalidate_forces_a_new_az_run() {
        let dir = tempfile::tempdir().unwrap();
        let (program, counter) = fake_az(dir.path(), 3600);
        let src = AzCliTokenSource::with_program(program);
        src.token(Audience::Arm).await.unwrap();
        src.token(Audience::Arm).await.unwrap();
        assert_eq!(count_lines(&counter), 1);
        src.invalidate(Audience::Arm);
        src.token(Audience::Arm).await.unwrap();
        assert_eq!(count_lines(&counter), 2);
    }
}
