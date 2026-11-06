# appi-dashboard Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build `appi-dashboard`, a btop-style Rust terminal app that shows near-real-time Application Insights data for any instance the user's `az login` can see. It has Overview and Failures tabs and lets you drill down from a failure spike to a single failed request.

**Architecture:** One binary crate. A tokio "data service" fetches from Azure (Resource Graph, Azure Monitor Metrics, the App Insights query API) using tokens from the Azure CLI, and sends results over channels to a pure, synchronous `App` state machine. ratatui draws `App` only when input, data, a resize or the 1 Hz clock tick arrives. A `Backend` trait has three implementations (Azure, a deterministic `--demo` generator, and test fakes), so all logic and UI can be tested without Azure.

**Tech Stack:** Rust edition 2024 (stable). Crates: ratatui and crossterm (`event-stream` feature), tokio, reqwest (rustls), serde, serde_json, chrono, url, clap (derive), directories, toml, async-trait, thiserror, unicode-width, futures, arboard, open, flate2, base64. Dev-only: wiremock, insta, tempfile.

**Spec:** `docs/superpowers/specs/2026-11-04-appi-dashboard-design.md`, with the visual mockup `2026-11-04-appi-dashboard-mockup.html` next to it. Executors read both.

## Global Constraints

- Platforms: Linux, macOS and Windows. CI runs `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test --all-targets` on ubuntu-latest, macos-latest and windows-latest.
- Auth only through the signed-in Azure CLI: `az account get-access-token --resource <audience> --output json`. The program is `az.cmd` on Windows and `az` elsewhere. Never store, log or print token values.
- Read-only toward Azure. The only calls are Resource Graph query POSTs, Metrics GETs and App Insights query POSTs.
- Cost is not a constraint, so add no request caps, budgets or counters. Throttling is a defect: honor 429 with `Retry-After`, and never run two dashboard refreshes at once.
- Refresh interval steps in seconds: 15, 30 (default), 60, 120, 300. Time ranges: 30m, 1h, 4h (default), 12h, 24h.
- Tab and Shift+Tab switch between Overview and Failures. The app never switches tabs by itself. The Failures tab label pulses red while the failure alert is active.
- Never panic on external data or terminal size. No `unwrap`, `expect` or direct indexing on API data. Truncate strings by display width (`unicode-width`), never by byte slicing.
- Redraw only on input, data, resize, or the 1 Hz clock tick.
- 24-bit color with automatic fallback to 256 colors. The app paints its own background (`#0f111a`) so it looks the same on light and dark terminals.
- reqwest with `default-features = false` and features `json` and `rustls-tls`, so there is no OpenSSL dependency.
- Public-repo hygiene: no tenant, subscription or dashboard IDs and no internal resource names in code, fixtures, docs or commit messages. `scripts/check-scrub.sh` must pass before every commit. Fixtures and demo data use made-up names such as `demo-app-prod` and `POST /api/orders`, and all-zero GUIDs.
- License `MIT OR Apache-2.0`. Crate and binary name `appi-dashboard`. Repo `https://github.com/suomiton/appi-dashboard` (public).
- Every commit message ends with a blank line and `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Review Focus

1. **An instance with no browser telemetry, no availability tests or no performance counters**, such as a Functions app. Affected panels and tiles show "no data in range" or "—", never zero-filled charts, errors or panics. Pinned in Task 7 (empty results stored as no-data) and Task 13 (render).
2. **Token expiry during an all-day session, and a slow `az`** (1–5 s, slower on Windows). Tokens refresh silently 5 minutes before expiry, concurrent callers share one `az` run, and the UI never blocks. Pinned in Task 2.
3. **Very long or non-ASCII names** (300-character operation names, CJK, emoji) on an 80×24 terminal. They are cut with "…" by display width and nothing panics. Pinned in Tasks 5, 13 and 16.
4. **Network loss, 429 or 5xx mid-refresh with several terminals open.** The last good data stays visible and marked stale, retries back off, and a tick never starts a second refresh while one is running. Pinned in Tasks 3, 7 and 12.
5. **Two terminals saving config at the same moment.** The file is never corrupt and always parses. Pinned in Task 8.

---

## File Structure

```
Cargo.toml  LICENSE-MIT  LICENSE-APACHE  README.md  .gitignore
scripts/check-scrub.sh             fails if any tracked file contains a term from the untracked .scrub-list
.github/workflows/ci.yml           fmt + clippy + test on 3 OSes
.github/workflows/release.yml      tag v* → 5 platform archives on GitHub Releases
src/main.rs                        parse CLI, dispatch run / probe
src/cli.rs                         clap definitions
src/config.rs                      Config TOML, default path, atomic save, recent instances, labels
src/probe.rs                       `probe` diagnostics subcommand
src/app.rs                         App state machine: views, focus, refresh schedule, picker, drill stack, keys → Commands
src/run.rs                         terminal setup, event loop, data service task, clipboard / open-URL effects
src/azure/mod.rs
src/azure/auth.rs                  TokenSource trait, AzCliTokenSource (cache, single-flight), StaticTokenSource
src/azure/http.rs                  AzureHttp: bearer auth, retry/backoff, AzError
src/azure/discovery.rs             Resource Graph → Vec<AppInstance>
src/azure/metrics.rs               Azure Monitor Metrics client
src/azure/logs.rs                  App Insights query client, Table
src/data/mod.rs
src/data/time.rs                   TimeRange, Window, grains, refresh steps, resample
src/data/format.rs                 number/byte/duration formatting, truncate_width, fmt_age
src/data/catalog.rs                SeriesId, metric groups
src/data/kql.rs                    KQL builders, kql_string escaping
src/data/backend.rs                Backend trait, AzureBackend
src/data/demo.rs                   DemoBackend (deterministic fake data)
src/data/dashboard.rs              View, DashboardUpdate, refresh_dashboard()
src/data/store.rs                  Store (last good values, staleness)
src/data/alerts.rs                 AlertConfig, evaluate()
src/data/drill.rs                  DrillTarget, ScreenData, loaders, transaction tree, stack frames, portal links
src/ui/mod.rs                      draw() dispatch, header, footer, help, auth / too-small states
src/ui/theme.rs                    palette, gradients, color depth detection, rgb → 256
src/ui/layout.rs                   responsive rectangles per view
src/ui/overview.rs  src/ui/failures.rs  src/ui/picker.rs  src/ui/drill.rs
src/ui/widgets/{mod,braille,bigdigits,meter,panel}.rs
tests/fixtures/*.json              made-up API responses
```

Unit tests go in `#[cfg(test)] mod tests` inside each file. insta snapshots live in each module's `snapshots/` directory. Test-only fakes go in `src/data/backend.rs` under `#[cfg(test)] pub mod fake` and are reused by later tasks: `FakeBackend` with canned responses keyed by metric name or KQL text, which also records the calls it receives.

---

### Task 1: Scaffold, repo hygiene and CI

**Files:**
- Create: `Cargo.toml`, `src/main.rs`, `.gitignore`, `LICENSE-MIT`, `LICENSE-APACHE`, `README.md`, `scripts/check-scrub.sh`, `.github/workflows/ci.yml`
- Modify: `docs/superpowers/specs/2026-11-04-appi-dashboard-design.md`, `docs/superpowers/specs/2026-11-04-appi-dashboard-mockup.html` (scrub)

**Interfaces:**
- Produces: crate `appi-dashboard` (bin) and `scripts/check-scrub.sh` (exit 0 means clean).

- [ ] **Step 1:** Run `git init -b main`. Write `.gitignore` with `/target`, `.superpowers/`, `.playwright-mcp/`, `.scrub-list`, `.dev-dashboard-id` and `*.local.toml`. The planning session already created the untracked files `.scrub-list` (internal terms, one per line) and `.dev-dashboard-id` (the reference dashboard's resource ID). Never copy their contents into tracked files.
- [ ] **Step 2:** Write `scripts/check-scrub.sh` (bash, `set -euo pipefail`). If `.scrub-list` is missing, print `check-scrub: no .scrub-list, skipping` and exit 0. Otherwise run `git ls-files -co --exclude-standard -z | xargs -0 grep -n -i -F -f .scrub-list`, print any matches, and exit 1 if there were any. `chmod +x`.
- [ ] **Step 3:** Run `scripts/check-scrub.sh`. Expected: FAIL, with matches in the spec and the mockup.
- [ ] **Step 4:** Scrub the spec and the mockup. Replace every listed internal resource name with the matching made-up name (`my-app-prod`, `my-func-prod`, `my-redis-prod`, `my-sql-prod`, `appdb`), and the company name with "the author's company". Run `scripts/check-scrub.sh` again. Expected: PASS.
- [ ] **Step 5:** Run `cargo init --name appi-dashboard` and set `edition = "2024"`. Add the dependencies from the Tech Stack with `cargo add`: tokio `rt-multi-thread,macros,time,process,sync`; reqwest `--no-default-features -F json,rustls-tls`; chrono `serde`; clap `derive`; crossterm `event-stream`. Run `cargo tree -i crossterm`. Expected: exactly one crossterm version, the same one ratatui uses. Pin crossterm to that version if needed.
- [ ] **Step 6:** In `src/main.rs`, print the version for now. Add the test `version_is_set()` asserting `!env!("CARGO_PKG_VERSION").is_empty()`. Run `cargo test`. Expected: PASS. `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` must also be clean.
- [ ] **Step 7:** Add the standard MIT and Apache-2.0 license texts (copyright line: `Copyright (c) 2026 Toni Suominen`) and a `README.md` stub (one paragraph plus "work in progress").
- [ ] **Step 8:** Write `.github/workflows/ci.yml`. It triggers on push and pull_request. Use a matrix over `[ubuntu-latest, macos-latest, windows-latest]` with steps `actions/checkout@v4`, `dtolnay/rust-toolchain@stable` (components `clippy, rustfmt`), `Swatinem/rust-cache@v2`, `cargo fmt --check` (ubuntu only), `cargo clippy --all-targets -- -D warnings` and `cargo test --all-targets`.
- [ ] **Step 9:** Run `scripts/check-scrub.sh`, then `git add -A`, then commit with `chore: scaffold crate, CI, licenses and design docs`.

---

### Task 2: Azure CLI token source

**Files:**
- Create: `src/azure/mod.rs`, `src/azure/auth.rs`
- Modify: `src/main.rs` (`mod azure;`)

**Interfaces:**
- Produces:
```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Audience { Arm, AppInsights }
impl Audience { pub fn resource(self) -> &'static str } // "https://management.azure.com", "https://api.applicationinsights.io"

#[derive(Clone, Debug, thiserror::Error)]
pub enum AuthError {
    #[error("Azure CLI (az) not found")] AzNotInstalled,
    #[error("not signed in: {0}")] NotLoggedIn(String),
    #[error("az failed: {0}")] Other(String),
}

#[async_trait::async_trait]
pub trait TokenSource: Send + Sync {
    async fn token(&self, aud: Audience) -> Result<String, AuthError>;
    fn invalidate(&self, aud: Audience);
}
pub struct AzCliTokenSource { /* private */ }
impl AzCliTokenSource { pub fn new() -> Self; pub fn with_program(program: impl Into<String>) -> Self }
pub struct StaticTokenSource(pub String);        // always Ok(self.0.clone()); invalidate is a no-op

pub(crate) struct CachedToken { pub access_token: String, pub expires_at: chrono::DateTime<chrono::Utc> }
pub(crate) fn parse_az_token(json: &str, now: DateTime<Utc>) -> Result<CachedToken, AuthError>;
pub(crate) fn classify_az_failure(stderr: &str) -> AuthError;
```

Rules:
- **Expiry:** use `expires_on` (epoch seconds, number or string) if present. Otherwise parse `expiresOn` (`%Y-%m-%d %H:%M:%S%.f`, local time). If neither parses, use now + 5 min.
- **Reuse:** reuse the cached token while `now < expires_at − 5 min`.
- **Single flight:** keep one `tokio::sync::Mutex` per audience and hold it across the `az` run.
- **Running az:** use `tokio::process::Command` with args `account get-access-token --resource <aud.resource()> --output json`, stdin null, `kill_on_drop(true)` and a 60 s timeout (timeout → `Other("az timed out")`). The default program is `az.cmd` on Windows (`cfg!(windows)`) and `az` elsewhere.
- **Errors:** a spawn error with `ErrorKind::NotFound` → `AzNotInstalled`. A non-zero exit → `classify_az_failure(stderr)`: stderr containing `az login`, `AADSTS` or `expired` (case-insensitive) → `NotLoggedIn(first non-empty stderr line)`, else `Other(that line)`.
- **Invalidate:** `invalidate` drops the cached token for that audience.

- [ ] **Step 1: Write failing tests** in `auth.rs`:
```rust
#[test] fn parses_epoch_expiry() {
    let t = parse_az_token(r#"{"accessToken":"tok","expires_on":1793800000,"expiresOn":"2026-11-04 16:00:00.000000"}"#, now()).unwrap();
    assert_eq!(t.access_token, "tok"); assert_eq!(t.expires_at.timestamp(), 1793800000);
}
#[test] fn falls_back_to_local_expires_on() { /* only expiresOn "2026-11-04 16:00:00.000000" → == Local 2026-11-04 16:00:00 converted to Utc */ }
#[test] fn missing_expiry_means_five_minutes() { /* {"accessToken":"t"} → expires_at == now + 5min */ }
#[test] fn classifies_not_logged_in() {
    assert!(matches!(classify_az_failure("ERROR: Please run 'az login' to setup account."), AuthError::NotLoggedIn(_)));
    assert!(matches!(classify_az_failure("ERROR: something else"), AuthError::Other(_)));
}
#[tokio::test] async fn missing_program_is_not_installed() {
    let s = AzCliTokenSource::with_program("definitely-not-az-xyz");
    assert!(matches!(s.token(Audience::Arm).await, Err(AuthError::AzNotInstalled)));
}
#[cfg(unix)] #[tokio::test] async fn concurrent_callers_share_one_az_run() {
    // fake az = temp shell script: sleep 1; echo x >> $COUNTER; print token JSON with expires_on = now+3600
    // 5 concurrent token(Arm) → all Ok("tok"), counter file has exactly 1 line
}
#[cfg(unix)] #[tokio::test] async fn refreshes_within_five_minutes_of_expiry() {
    // script prints expires_on = now+240 → two sequential token() calls → counter has 2 lines
}
```
- [ ] **Step 2:** Run `cargo test azure::auth`. Expected: FAIL (items not defined).
- [ ] **Step 3:** Implement the interface above in `src/azure/auth.rs`.
- [ ] **Step 4:** Run `cargo test azure::auth`. Expected: PASS. Clippy clean.
- [ ] **Step 5:** Run `scripts/check-scrub.sh`, then commit with `feat(azure): Azure CLI token source with caching and single-flight`.

---

### Task 3: Azure HTTP client with retry

**Files:**
- Create: `src/azure/http.rs`

**Interfaces:**
- Consumes: `TokenSource`, `Audience`, `AuthError` (Task 2).
- Produces:
```rust
#[derive(Clone, Debug)] pub struct Endpoints { pub arm: url::Url, pub app_insights: url::Url }
impl Default for Endpoints // "https://management.azure.com/", "https://api.applicationinsights.io/"
#[derive(Clone, Debug)] pub struct RetryPolicy { pub max_attempts: u32, pub base_delay: Duration, pub max_delay: Duration }
impl Default for RetryPolicy // 3, 1s, 30s

#[derive(Clone, Debug, thiserror::Error)]
pub enum AzError {
    #[error(transparent)] Auth(#[from] AuthError),
    #[error("throttled by Azure")] Throttled { retry_after: Option<Duration> },
    #[error("HTTP {status}: {message}")] Http { status: u16, message: String },
    #[error("network: {0}")] Network(String),
    #[error("unexpected response: {0}")] Decode(String),
}
#[derive(Clone)] pub struct AzureHttp { /* reqwest::Client, Arc<dyn TokenSource>, Endpoints, RetryPolicy */ }
impl AzureHttp {
    pub fn new(tokens: Arc<dyn TokenSource>, endpoints: Endpoints) -> Self;
    pub fn with_retry(self, retry: RetryPolicy) -> Self;
    pub fn endpoints(&self) -> &Endpoints;
    pub async fn get_json<T: DeserializeOwned>(&self, aud: Audience, url: Url) -> Result<T, AzError>;
    pub async fn post_json<B: Serialize + ?Sized, T: DeserializeOwned>(&self, aud: Audience, url: Url, body: &B) -> Result<T, AzError>;
}
pub(crate) fn azure_error_message(body: &str) -> String; // {"error":{"code","message"}} → "code: message", else first 200 chars
```

Behavior:
- The client has a 60 s timeout and user agent `appi-dashboard/<version>`.
- **429:** sleep `Retry-After` seconds (capped at `max_delay`), or the backoff if the header is absent, then retry. Out of attempts → `Throttled`.
- **5xx and network errors:** exponential backoff `base_delay * 2^(attempt-1)`, capped. Out of attempts → `Http` or `Network`.
- **401:** call `tokens.invalidate(aud)` once and retry (this counts as an attempt).
- **Other 4xx:** return `Http` immediately.
- **Bad JSON:** return `Decode`.

- [ ] **Step 1: Write failing tests** using wiremock, `StaticTokenSource("t")` and `RetryPolicy { max_attempts: 3, base_delay: 10ms, max_delay: 50ms }`:
  - `sends_bearer_token`: the mock requires header `Authorization: Bearer t` and returns 200 `{"ok":true}`. Expect `Ok`.
  - `retries_429_honoring_retry_after`: 429 with `Retry-After: 0`, then 200. Expect `Ok`, with 2 requests received.
  - `gives_up_after_three_5xx`: 503 three times. Expect `Err(Http{status:503,..})`, with 3 requests received.
  - `returns_4xx_without_retry`: 403 with body `{"error":{"code":"AuthorizationFailed","message":"no access"}}`. Expect `Err(Http{status:403, message:"AuthorizationFailed: no access"})`, with 1 request received.
  - `invalidates_token_on_401`: use a counting TokenSource double. 401 then 200 gives `Ok`, and invalidate is called once.
  - `decode_error_is_reported`: 200 with body `not json`. Expect `Err(Decode(_))`.
- [ ] **Step 2:** Run `cargo test azure::http`. Expected: FAIL.
- [ ] **Step 3:** Implement `AzureHttp` in `src/azure/http.rs`.
- [ ] **Step 4:** Run `cargo test azure::http`. Expected: PASS.
- [ ] **Step 5:** Run the scrub check, then commit with `feat(azure): HTTP client with bearer auth and retry`.

---

### Task 4: Azure API clients (discovery, metrics, logs)

**Files:**
- Create: `src/azure/discovery.rs`, `src/azure/metrics.rs`, `src/azure/logs.rs`
- Create: `tests/fixtures/resource_graph_page1.json`, `resource_graph_page2.json`, `metrics_count.json`, `metrics_split.json`, `logs_table.json` (made-up data, all-zero GUIDs)

**Interfaces:**
- Consumes: `AzureHttp`, `AzError`, `Audience` (Tasks 2–3).
- Produces:
```rust
// discovery.rs
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AppInstance { pub name: String, pub resource_id: String, pub resource_group: String, pub subscription_id: String,
    pub subscription_name: String, pub tenant_id: String, pub app_id: String, pub location: String }
pub async fn list_instances(http: &AzureHttp) -> Result<Vec<AppInstance>, AzError>;

// metrics.rs
pub const METRICS_API_VERSION: &str = "2023-10-01";
#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum Aggregation { Average, Count, Total, Maximum, Minimum } // as_str(): lowercase
#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum Interval { PT1M, PT5M, PT15M, PT30M, Full }           // as_str(): "PT1M".."FULL"
#[derive(Clone, Debug, PartialEq)] pub struct MetricsRequest { pub resource_id: String, pub metric_names: Vec<String>,
    pub aggregation: Aggregation, pub start: DateTime<Utc>, pub end: DateTime<Utc>, pub interval: Interval, pub split_by: Option<String> }
#[derive(Clone, Debug, PartialEq)] pub struct MetricSeries { pub name: String, pub dimension: Option<String>, pub points: Vec<(DateTime<Utc>, Option<f64>)> }
pub fn metrics_url(arm: &Url, req: &MetricsRequest) -> Url;
pub async fn fetch_metrics(http: &AzureHttp, req: &MetricsRequest) -> Result<Vec<MetricSeries>, AzError>;

// logs.rs
#[derive(Clone, Debug, Default, PartialEq)] pub struct Table { pub columns: Vec<String>, pub rows: Vec<Vec<serde_json::Value>> }
impl Table {
    pub fn len(&self) -> usize; pub fn is_empty(&self) -> bool;
    pub fn str(&self, row: usize, col: &str) -> Option<&str>;
    pub fn f64(&self, row: usize, col: &str) -> Option<f64>;          // JSON number or numeric string
    pub fn time(&self, row: usize, col: &str) -> Option<DateTime<Utc>>; // RFC 3339 string
    pub fn json(&self, row: usize, col: &str) -> Option<&serde_json::Value>;
}
pub async fn query(http: &AzureHttp, app_id: &str, kql: &str, timespan: &str) -> Result<Table, AzError>;
```

Details:
- **Discovery:** `POST {arm}providers/Microsoft.ResourceGraph/resources?api-version=2022-10-01` (audience Arm). Body: `{"query": Q, "options": {"resultFormat": "objectArray", "$top": 1000}}`, adding `"$skipToken"` while the response has one. Skip rows with an empty `appId`. `Q` exactly:
```
resources
| where type =~ 'microsoft.insights/components'
| project name, id, resourceGroup, subscriptionId, tenantId, location, appId = tostring(properties.AppId)
| join kind=leftouter (resourcecontainers | where type =~ 'microsoft.resources/subscriptions' | project subscriptionId, subscriptionName = name) on subscriptionId
| project name, id, resourceGroup, subscriptionId, subscriptionName, tenantId, location, appId
| order by name asc
```
- **Metrics:** `GET {arm}{resource_id without leading '/'}/providers/Microsoft.Insights/metrics` (audience Arm). Query pairs: `api-version`, `metricnames` (comma-joined), `aggregation`, `timespan` = `{start}/{end}` (RFC 3339 with `Z`, seconds precision), `interval`, plus `$filter` = `{dim} eq '*'` when `split_by` is set. To parse the response: `value[].name.value` is the metric name; `timeseries[].metadatavalues[0].value` is the dimension; `timeseries[].data[]` gives `timeStamp` plus the field named by the aggregation (missing → `None`).
- **Logs:** `POST {app_insights}v1/apps/{app_id}/query` (audience AppInsights) with body `{"query": kql, "timespan": timespan}`. Use `tables[0]`.

- [ ] **Step 1: Write failing tests** (wiremock plus fixtures):
  - `discovery_follows_skip_token`: page 1 returns 2 rows and `$skipToken: "abc"`; page 2 returns 1 row. Expect 3 instances, and the second request body contains `"$skipToken":"abc"`.
  - `discovery_skips_rows_without_app_id`.
  - `metrics_url_has_expected_pairs`: parse `metrics_url(...)` back with `query_pairs()`. It must contain `("metricnames","requests/failed,requests/count")`, `("aggregation","count")`, `("interval","PT5M")`, `("timespan","2026-11-04T10:00:00Z/2026-11-04T14:00:00Z")` and `("$filter","cloud/roleInstance eq '*'")`.
  - `metrics_parses_points_and_missing_values`: expect `points == [(t0, Some(3.0)), (t1, None)]`.
  - `metrics_parses_split_dimension`: expect two series with `dimension == Some("demo-host-0")` and `Some("demo-host-1")`.
  - `logs_parses_table`: `str`, `f64` (number and the string `"3.38"`) and `time` work; a missing column → `None`; an out-of-range row → `None`.
  - `logs_sends_timespan`: the request body JSON has `"timespan":"PT4H"`.
- [ ] **Step 2:** Run `cargo test azure::`. Expected: the new tests FAIL.
- [ ] **Step 3:** Implement the three modules.
- [ ] **Step 4:** Run `cargo test azure::`. Expected: PASS.
- [ ] **Step 5:** Run the scrub check, then commit with `feat(azure): resource graph discovery, metrics and logs clients`.

---

### Task 5: Time ranges and formatting helpers

**Files:**
- Create: `src/data/mod.rs`, `src/data/time.rs`, `src/data/format.rs`

**Interfaces:**
- Consumes: `Interval` (Task 4).
- Produces:
```rust
// time.rs
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize)]
pub enum TimeRange { #[serde(rename="30m")] M30, #[serde(rename="1h")] H1, #[default] #[serde(rename="4h")] H4,
                     #[serde(rename="12h")] H12, #[serde(rename="24h")] H24 }
impl TimeRange {
    pub const ALL: [TimeRange; 5];
    pub fn minutes(self) -> i64; pub fn grain(self) -> Interval; pub fn grain_minutes(self) -> i64;
    pub fn label(self) -> &'static str;  // "30m","1h","4h","12h","24h"
    pub fn iso(self) -> &'static str;    // "PT30M","PT1H","PT4H","PT12H","PT24H"
    pub fn next(self) -> Self; pub fn parse(s: &str) -> Option<Self>;
}
#[derive(Clone, Copy, Debug, PartialEq)] pub struct Window { pub start: DateTime<Utc>, pub end: DateTime<Utc> }
pub fn window(range: TimeRange, now: DateTime<Utc>) -> Window; // end = now truncated to the minute
pub const REFRESH_STEPS: [u64; 5] = [15, 30, 60, 120, 300];
pub const DEFAULT_REFRESH: u64 = 30;
pub fn refresh_step(current: u64, up: bool) -> u64;
pub fn resample(values: &[Option<f64>], n: usize) -> Vec<Option<f64>>; // nearest: out[i] = values[i * len / n]

// format.rs
pub fn fmt_big(v: f64) -> (String, &'static str); // 3 significant digits, ≤ 2 decimals, trailing zeros trimmed; suffix "", "k", "M", "G"
pub fn fmt_num(v: f64) -> String;                 // fmt_big joined
pub fn fmt_ms(ms: f64) -> String;                 // <1000 "{:.0}ms"; <10000 "{:.2}s"; else "{:.1}s"
pub fn fmt_bytes(b: f64) -> String;               // 1024-based, units B KB MB GB TB, 3 significant digits
pub fn fmt_rate_bytes(bps: f64) -> String;        // fmt_bytes + "/s"
pub fn fmt_age(d: chrono::Duration) -> String;    // "45s", "3m", "2h"
pub fn truncate_width(s: &str, max: usize) -> String; // by display width, appends "…" when cut; max 0 → ""
```
Grains: 30m → PT1M, 1h → PT1M, 4h → PT5M, 12h → PT5M, 24h → PT15M. `next` cycles 30m → 1h → 4h → 12h → 24h → 30m. `refresh_step` snaps a value that isn't a step up to the next higher step, and clamps at both ends.

- [ ] **Step 1: Write failing tests:**
```rust
assert_eq!(TimeRange::H4.grain(), Interval::PT5M); assert_eq!(TimeRange::H24.grain(), Interval::PT15M);
assert_eq!(TimeRange::H24.next(), TimeRange::M30); assert_eq!(TimeRange::parse("12h"), Some(TimeRange::H12)); assert_eq!(TimeRange::parse("2h"), None);
// window(H1, 2026-11-04T14:03:27Z) == 13:03:00Z..14:03:00Z
assert_eq!(refresh_step(30, true), 60); assert_eq!(refresh_step(300, true), 300); assert_eq!(refresh_step(15, false), 15); assert_eq!(refresh_step(45, true), 60);
assert_eq!(resample(&[Some(1.), Some(2.)], 4), vec![Some(1.), Some(1.), Some(2.), Some(2.)]);
assert_eq!(resample(&[Some(1.), Some(2.), Some(3.), Some(4.)], 2), vec![Some(1.), Some(3.)]);
assert_eq!(resample(&[], 3), vec![None, None, None]);
assert_eq!(fmt_big(165.), ("165".into(), "")); assert_eq!(fmt_big(119.48), ("119".into(), ""));
assert_eq!(fmt_big(11.72), ("11.7".into(), "")); assert_eq!(fmt_big(0.3137), ("0.31".into(), ""));
assert_eq!(fmt_big(8_330_000.), ("8.33".into(), "M")); assert_eq!(fmt_big(12_900.), ("12.9".into(), "k"));
assert_eq!(fmt_big(127_060_000.), ("127".into(), "M")); assert_eq!(fmt_big(0.), ("0".into(), ""));
assert_eq!(fmt_ms(119.48), "119ms"); assert_eq!(fmt_ms(1500.), "1.50s"); assert_eq!(fmt_ms(30020.), "30.0s");
assert_eq!(fmt_bytes(11.72 * 1024f64.powi(3)), "11.7 GB"); assert_eq!(fmt_bytes(512.), "512 B");
```
  Review Focus 3 tests:
  - `truncate_width("POST /api/import/upload", 10)` has width ≤ 10 and ends with `…`.
  - `truncate_width("日本語テキスト", 5)` has width ≤ 5.
  - `truncate_width(&("x".repeat(300) + "😀"), 20)` has width ≤ 20.
  - `truncate_width("ab", 5) == "ab"` and `truncate_width("abc", 0) == ""`.
- [ ] **Step 2:** Run `cargo test data::`. Expected: FAIL.
- [ ] **Step 3:** Implement both modules.
- [ ] **Step 4:** Run `cargo test data::`. Expected: PASS.
- [ ] **Step 5:** Run the scrub check, then commit with `feat(data): time ranges, resampling and formatting helpers`.

---

### Task 6: Metric catalog and dashboard KQL

**Files:**
- Create: `src/data/catalog.rs`, `src/data/kql.rs`

**Interfaces:**
- Consumes: `TimeRange` (Task 5).
- Produces:
```rust
// catalog.rs
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SeriesId { RequestsCount, RequestsFailed, ExceptionsServer, ExceptionsBrowser, DependenciesFailed, AvailabilityCount,
    ServerResponse, PageLoadTotal, PageLoadNetwork, PageLoadProcessing, PageLoadSend, ProcessCpu, ProcessorCpu,
    MemoryAvailable, ProcessIo, Availability, Users, Sessions }
pub struct MetricSpec { pub series: SeriesId, pub metric: &'static str }
pub const COUNT_METRICS: &[MetricSpec];   // fetched with Aggregation::Count
pub const AVERAGE_METRICS: &[MetricSpec]; // fetched with Aggregation::Average
pub const ALERT_METRICS: &[MetricSpec];   // Count, PT1M, last 10 minutes
pub fn series_for(metric: &str) -> Option<SeriesId>;

// kql.rs
pub const ALL_TABLES: &str = "union traces, requests, pageViews, dependencies, customEvents, availabilityResults, exceptions, customMetrics, browserTimings";
pub fn kql_string(s: &str) -> String;  // "..." with \ → \\, " → \", newline → \n
pub fn summary_scalars() -> String;
pub fn users_sessions_series(range: TimeRange) -> String;
pub fn failure_tops() -> String;
pub fn failing_endpoints(range: TimeRange) -> String;
pub fn endpoint_exceptions(name: &str) -> String;
pub fn endpoint_dependencies(name: &str) -> String;
pub fn endpoint_recent_failures(name: &str) -> String;
pub fn request_row(operation_id: &str, request_id: Option<&str>) -> String;
pub fn request_exceptions(operation_id: &str) -> String;
pub fn transaction(operation_id: &str) -> String;
```
Catalog values (metric names exactly as in Microsoft's supported-metrics list for `microsoft.insights/components`):
- **COUNT:** `requests/count` → RequestsCount, `requests/failed` → RequestsFailed, `exceptions/server` → ExceptionsServer, `exceptions/browser` → ExceptionsBrowser, `dependencies/failed` → DependenciesFailed, `availabilityResults/count` → AvailabilityCount.
- **AVERAGE:** `requests/duration` → ServerResponse, `browserTimings/totalDuration` → PageLoadTotal, `browserTimings/networkDuration` → PageLoadNetwork, `browserTimings/processingDuration` → PageLoadProcessing, `browserTimings/sendDuration` → PageLoadSend, `performanceCounters/processCpuPercentage` → ProcessCpu, `performanceCounters/processorCpuPercentage` → ProcessorCpu, `performanceCounters/memoryAvailableBytes` → MemoryAvailable, `performanceCounters/processIOBytesPerSecond` → ProcessIo, `availabilityResults/availabilityPercentage` → Availability.
- **ALERT:** `requests/failed`, `exceptions/server`, `dependencies/failed`.
- **Users and Sessions** come from KQL, matching how the portal defines them as log-based metrics.

KQL. `{g}` is `range.grain_minutes()`; `{X}` is `kql_string(name)`; `{O}` is `kql_string(operation_id)`; `{R}` is `kql_string(request_id)`. Use these exactly:
```
// summary_scalars
let all = <ALL_TABLES>;
print users_5m = toscalar(all | where timestamp > ago(5m) and notempty(user_Id) | summarize dcount(user_Id)),
      users_range = toscalar(all | where notempty(user_Id) | summarize dcount(user_Id)),
      sessions_range = toscalar(all | where notempty(session_Id) | summarize dcount(session_Id)),
      servers_10m = toscalar(union requests, performanceCounters | where timestamp > ago(10m) and notempty(cloud_RoleInstance) | summarize dcount(cloud_RoleInstance)),
      response_p95_ms = toscalar(requests | summarize percentile(duration, 95))
// users_sessions_series
<ALL_TABLES>
| summarize users = dcountif(user_Id, notempty(user_Id)), sessions = dcountif(session_Id, notempty(session_Id)) by bin(timestamp, {g}m)
| order by timestamp asc
// failure_tops
let eps = requests | where timestamp > ago(15m) and tostring(success) == "False" | summarize n = sum(itemCount) by name | top 5 by n | extend kind = "endpoint", detail = "";
let exs = exceptions | where timestamp > ago(15m) and client_Type != "Browser" | summarize n = sum(itemCount), detail = take_any(outerMessage) by name = type | top 5 by n | extend kind = "exception";
union eps, exs | project kind, name, detail, n
// failing_endpoints
requests | where tostring(success) == "False"
| summarize n = sum(itemCount) by name, resultCode, bin(timestamp, {g}m)
// endpoint_exceptions
exceptions | where operation_Name == {X} | summarize n = sum(itemCount), detail = take_any(outerMessage) by type | top 10 by n
// endpoint_dependencies
dependencies | where operation_Name == {X} and tostring(success) == "False" | summarize n = sum(itemCount) by type, target | top 10 by n
// endpoint_recent_failures
requests | where name == {X} and tostring(success) == "False"
| top 50 by timestamp desc
| project timestamp, resultCode, duration, cloud_RoleInstance, operation_Id, id
| join kind=leftouter (exceptions | where operation_Name == {X} | summarize exType = take_any(type) by operation_Id) on operation_Id
| project timestamp, resultCode, duration, cloud_RoleInstance, operation_Id, id, exType
| order by timestamp desc
// request_row  (the "and id == {R}" part only when request_id is Some)
requests | where operation_Id == {O} and id == {R} | top 1 by timestamp desc
| project timestamp, name, resultCode, duration, success = tostring(success), cloud_RoleInstance, operation_Id, id, session_Id, user_Id
// request_exceptions
exceptions | where operation_Id == {O} | top 5 by timestamp asc | project timestamp, type, outerMessage, innermostMessage, details
// transaction
union requests, dependencies | where operation_Id == {O}
| project timestamp, itemType, name, type, target, duration, success = tostring(success), resultCode, id, operation_ParentId
| order by timestamp asc | take 200
```

- [ ] **Step 1: Write failing tests:**
  - `kql_string(r#"GET /a"b\c"#) == r#""GET /a\"b\\c""#`.
  - `endpoint_recent_failures(r#"x" | take 1 //"#)` contains `name == "x\" | take 1 //"`, showing injection is neutralized.
  - `users_sessions_series(TimeRange::H4)` contains `bin(timestamp, 5m)`, and `users_sessions_series(TimeRange::M30)` contains `bin(timestamp, 1m)`.
  - Every metric name across the three groups is unique within its group. `COUNT_METRICS.len() <= 20` and `AVERAGE_METRICS.len() <= 20` (the API limit per call).
  - `series_for("requests/failed") == Some(SeriesId::RequestsFailed)` and `series_for("nope") == None`.
  - `request_row("o", None)` does not contain `id ==`.
  - insta snapshots of `summary_scalars()` and `failure_tops()`.
- [ ] **Step 2:** Run `cargo test data::`. Expected: the new tests FAIL.
- [ ] **Step 3:** Implement both modules.
- [ ] **Step 4:** Run `cargo test data::` and accept the snapshots (`cargo insta accept` after reviewing). Expected: PASS.
- [ ] **Step 5:** Run the scrub check, then commit with `feat(data): metric catalog and dashboard KQL builders`.

---

### Task 7: Backends, dashboard refresh, store and alerts

**Files:**
- Create: `src/data/backend.rs`, `src/data/demo.rs`, `src/data/dashboard.rs`, `src/data/store.rs`, `src/data/alerts.rs`

**Interfaces:**
- Consumes: Tasks 4–6.
- Produces:
```rust
// backend.rs
#[async_trait::async_trait]
pub trait Backend: Send + Sync {
    async fn instances(&self) -> Result<Vec<AppInstance>, AzError>;
    async fn metrics(&self, req: MetricsRequest) -> Result<Vec<MetricSeries>, AzError>;
    async fn query(&self, app_id: &str, kql: &str, timespan: &str) -> Result<Table, AzError>;
}
pub struct AzureBackend { /* AzureHttp */ } impl AzureBackend { pub fn new(http: AzureHttp) -> Self }
#[cfg(test)] pub mod fake { pub struct FakeBackend { /* canned by metric name / exact KQL text; records calls */ } }

// demo.rs
pub struct DemoBackend { /* seed */ } impl DemoBackend { pub fn new() -> Self }
pub fn demo_instances() -> Vec<AppInstance>; // "demo-app-prod", "demo-app-staging", subscription "Demo Subscription", all-zero GUIDs

// dashboard.rs
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)] pub enum View { Overview, Failures }
#[derive(Clone, Debug, Default, PartialEq)] pub struct Scalars { pub users_5m: Option<f64>, pub users_range: Option<f64>,
    pub sessions_range: Option<f64>, pub servers_10m: Option<f64>, pub response_p95_ms: Option<f64> }
#[derive(Clone, Debug, Default, PartialEq)] pub struct AlertWindow { pub failed_5m: f64, pub server_exceptions_5m: f64, pub dependency_failures_5m: f64 }
#[derive(Clone, Debug, PartialEq)] pub struct TopItem { pub name: String, pub detail: String, pub n: f64 }
#[derive(Clone, Debug, Default, PartialEq)] pub struct FailureTops { pub endpoints: Vec<TopItem>, pub exceptions: Vec<TopItem> }
#[derive(Clone, Debug, Default, PartialEq)] pub struct Series { pub points: Vec<(DateTime<Utc>, Option<f64>)> }
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)] pub enum Source { CountMetrics, AverageMetrics, CountTotals, AverageTotals, AlertWindow, Scalars, UsersSessions, Tops }
#[derive(Clone, Debug, Default)] pub struct DashboardUpdate { pub series: HashMap<SeriesId, Series>, pub totals: HashMap<SeriesId, f64>,
    pub scalars: Option<Scalars>, pub alert_window: Option<AlertWindow>, pub tops: Option<FailureTops>, pub errors: Vec<(Source, AzError)> }
pub async fn refresh_dashboard(backend: &dyn Backend, inst: &AppInstance, range: TimeRange, view: View, now: DateTime<Utc>) -> DashboardUpdate;

// store.rs
#[derive(Clone, Debug, PartialEq)] pub struct Timed<T> { pub value: T, pub at: DateTime<Utc> }
#[derive(Clone, Debug, Default)] pub struct Store { pub series: HashMap<SeriesId, Timed<Series>>, pub totals: HashMap<SeriesId, Timed<f64>>,
    pub scalars: Option<Timed<Scalars>>, pub alert_window: Option<Timed<AlertWindow>>, pub tops: Option<Timed<FailureTops>>,
    pub last_success: Option<DateTime<Utc>>, pub last_error: Option<(DateTime<Utc>, String)> }
impl Store {
    pub fn apply(&mut self, u: DashboardUpdate, at: DateTime<Utc>);
    pub fn clear(&mut self);
    pub fn has_data(&self, id: SeriesId) -> bool;       // series with ≥1 Some point, or a total
    pub fn total(&self, id: SeriesId) -> Option<f64>;
    pub fn age(&self, id: SeriesId, now: DateTime<Utc>) -> Option<chrono::Duration>;
    pub fn is_stale(&self, id: SeriesId, now: DateTime<Utc>, refresh_secs: u64) -> bool; // age > 2*refresh + 60s
}

// alerts.rs
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)] #[serde(default)]
pub struct AlertConfig { pub failed_requests_5m: f64, pub server_exceptions_5m: f64, pub dependency_failures_5m: f64,
    pub availability_below_pct: f64, pub response_avg_above_ms: f64 }  // Default: 5, 10, 10, 99.0, 1000.0
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)] pub enum AlertKind { Failures, ExceptionsDeps, Availability, Response }
#[derive(Clone, Debug, Default, PartialEq)] pub struct AlertState { pub active: HashSet<AlertKind> }
impl AlertState { pub fn failures(&self) -> bool; pub fn is(&self, k: AlertKind) -> bool }
pub fn evaluate(cfg: &AlertConfig, store: &Store) -> AlertState;
```
`refresh_dashboard` runs these concurrently (`tokio::join!`), all over `window(range, now)` unless stated:

| Source | Call | Result |
|---|---|---|
| CountMetrics | COUNT_METRICS, Count, `range.grain()` | series |
| AverageMetrics | AVERAGE_METRICS, Average, `range.grain()` | series |
| CountTotals | COUNT_METRICS, Count, `Interval::Full` | totals = sum of the points that are Some |
| AverageTotals | AVERAGE_METRICS, Average, `Interval::Full` | totals = the single point, if Some |
| AlertWindow | ALERT_METRICS, Count, PT1M, start = end − 10 min | each field = sum of points with ts ≥ end − 5 min |
| Scalars | `query(app_id, summary_scalars(), range.iso())` | Scalars |
| UsersSessions | `query(app_id, users_sessions_series(range), range.iso())` | series Users, Sessions |
| Tops (Failures view only) | `query(app_id, failure_tops(), "PT15M")` | FailureTops |

A failed call becomes an entry in `errors` and the other sources still fill in. A metric with all-`None` points is still stored as a `Series` (so `has_data` is false), and a missing total is simply not inserted.

`Store::apply` overwrites only the fields present in the update and keeps older values for the rest. That is how stale data stays visible. `last_success = at` only when `errors` is empty; `last_error` = the first error.

`evaluate` rules:
- `Failures` when `failed_5m >= failed_requests_5m`.
- `ExceptionsDeps` when either 5-minute count ≥ its threshold.
- `Availability` when the total exists and is < `availability_below_pct`.
- `Response` when the ServerResponse total exists and is > `response_avg_above_ms`.

`DemoBackend` is deterministic, with shapes like the mockup:
- Noisy sessions and users; a failure spike in the last 10 minutes (alert window `failed_5m = 12`); low, spiky CPU; availability 100.
- Tops: `POST /api/orders` 12, `GET /api/reports/export` 2, `POST /api/auth/token` 1. Exceptions: `SqlException` 9 ("Execution Timeout Expired"), `HttpRequestException` 3.
- It answers by matching metric names, and KQL text by comparing it to the builder outputs. Unknown KQL returns an empty `Table`.

- [ ] **Step 1: Write failing tests:**
  - `refresh_collects_all_sources`: FakeBackend with fixtures and `View::Failures`. Expect the RequestsFailed series to have N points, `totals[RequestsFailed] == 24.0`, `alert_window.failed_5m` equal to the sum of the last 5 one-minute points, `scalars.users_5m == Some(165.)` and `tops.endpoints[0].name == "POST /api/orders"`.
  - `overview_skips_tops`: with `View::Overview`, FakeBackend recorded no `failure_tops()` query and `update.tops` is None.
  - `partial_failure_keeps_other_parts`: AverageMetrics returns `Http{500}`. `errors` contains `(AverageMetrics, _)` and RequestsFailed is still present.
  - `store_keeps_old_values_and_marks_stale` (Review Focus 4): apply a full update at t0, then an update with only scalars plus errors at t0+5 min. The RequestsFailed series is still the t0 one, `is_stale(RequestsFailed, t0+5m, 30)` is true, and `last_success == Some(t0)`.
  - `empty_metric_is_no_data` (Review Focus 1): the browserTimings series is all None. `has_data(PageLoadTotal) == false` and nothing panics.
  - Alerts: `failed_5m` 5 is active and 4 is not; Availability 98.5 is active; no availability total is not active.
  - `demo_backend_trips_failure_alert`: `refresh_dashboard(&DemoBackend::new(), ...)`, apply, then `evaluate(...).failures() == true`.
- [ ] **Step 2:** Run `cargo test data::`. Expected: FAIL.
- [ ] **Step 3:** Implement the five modules.
- [ ] **Step 4:** Run `cargo test data::`. Expected: PASS.
- [ ] **Step 5:** Run the scrub check, then commit with `feat(data): backends, dashboard refresh, store and alerts`.

---

### Task 8: Config and CLI

**Files:**
- Create: `src/config.rs`, `src/cli.rs`

**Interfaces:**
- Consumes: `AppInstance`, `TimeRange`, `REFRESH_STEPS`, `AlertConfig`.
- Produces:
```rust
// config.rs
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SavedInstance { pub name: String, pub resource_id: String, pub app_id: String, pub tenant_id: String,
    pub subscription_name: String, pub resource_group: String }
impl From<&AppInstance> for SavedInstance; impl SavedInstance { pub fn to_instance(&self) -> AppInstance }
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")] pub enum ColorMode { #[default] Auto, Truecolor, #[serde(rename = "256")] #[value(name = "256")] Ansi256 }
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)] #[serde(default)]
pub struct Config { pub last_instance: Option<SavedInstance>, pub recent: Vec<SavedInstance>, pub refresh_secs: u64,
    pub range: TimeRange, pub color: ColorMode, pub alerts: AlertConfig, pub labels: BTreeMap<String, String> }
impl Default for Config // refresh 30, range 4h
impl Config { pub fn remember(&mut self, inst: &AppInstance); pub fn label_for(&self, resource_id: &str) -> Option<&str> }
pub fn default_path() -> Option<PathBuf>; // ProjectDirs::from("io.github", "suomiton", "appi-dashboard")?.config_dir().join("config.toml")
pub fn load(path: &Path) -> (Config, Option<String>); // missing → (default, None); unreadable/invalid → (default, Some(warning))
pub fn save(path: &Path, cfg: &Config) -> std::io::Result<()>;

// cli.rs
#[derive(clap::Parser, Debug)] #[command(name = "appi-dashboard", version, about)]
pub struct Cli {
    #[arg(long)] pub app: Option<String>,
    #[arg(long, value_parser = parse_range)] pub range: Option<TimeRange>,
    #[arg(long, value_parser = parse_refresh)] pub refresh: Option<u64>,
    #[arg(long)] pub config: Option<PathBuf>,
    #[arg(long)] pub demo: bool,
    #[arg(long, value_enum)] pub color: Option<ColorMode>,
    #[command(subcommand)] pub command: Option<Cmd>,
}
#[derive(clap::Subcommand, Debug)]
pub enum Cmd { Probe { #[arg(long)] app: Option<String>, #[arg(long, value_parser = parse_range)] range: Option<TimeRange> } }
pub fn parse_range(s: &str) -> Result<TimeRange, String>;
pub fn parse_refresh(s: &str) -> Result<u64, String>; // must be in REFRESH_STEPS; the error lists the allowed values
```
- `remember` moves the instance to the front of `recent` (deduplicated by lowercase resource ID), keeps at most 10, and sets `last_instance`.
- `labels` keys are lowercase resource IDs, and `label_for` lowercases its input.
- `save` creates parent directories, writes `config.toml.<pid>.<counter>.tmp` in the same directory, `sync_all`s it, then `rename`s it over the target. On `PermissionDenied` (Windows readers), it retries the rename up to 5 times with 20 ms sleeps.

- [ ] **Step 1: Write failing tests** (tempfile):
  - `load_missing_gives_default`.
  - `load_invalid_gives_default_and_warning`.
  - `save_load_roundtrip`.
  - `remember_dedupes_and_caps_at_10`: 12 different remembers plus one repeat. Expect `len == 10`, `recent[0]` is the latest, and the repeat is moved to the front.
  - `label_for_is_case_insensitive`.
  - `concurrent_saves_never_corrupt` (Review Focus 5): two threads each save 50 times (configs with refresh 15 and 300) while a third thread loads 100 times. Every load returns `None` as the warning, and the final file parses with refresh ∈ {15, 300}.
  - CLI: `["appi-dashboard","--range","12h","--refresh","60","--app","demo-app-prod"]` parses into those values. `--refresh 45` fails with an error mentioning `15, 30, 60, 120, 300`. `["appi-dashboard","probe","--app","x"]` gives `Cmd::Probe`.
  - `default_path_ends_with_config_toml`.
- [ ] **Step 2:** Run `cargo test config cli`. Expected: FAIL.
- [ ] **Step 3:** Implement both modules and wire `mod`s in `main.rs`, which parses `Cli` and still only prints the version.
- [ ] **Step 4:** Run `cargo test`. Expected: PASS.
- [ ] **Step 5:** Run the scrub check, then commit with `feat: config file and command-line interface`.

---

### Task 9: `probe` diagnostics and portal parity (USER CHECKPOINT)

**Files:**
- Create: `src/probe.rs`
- Modify: `src/main.rs` (dispatch `Cmd::Probe` on a tokio runtime; exit code 1 when the probe reports a failure)

**Interfaces:**
- Consumes: `TokenSource`, `Backend`, `refresh_dashboard`, `TimeRange`.
- Produces: `pub async fn run_probe(tokens: &dyn TokenSource, backend: &dyn Backend, app: Option<&str>, range: TimeRange, out: &mut dyn std::io::Write) -> std::io::Result<bool>` (true means no `[FAIL]` lines).

Output contract: one line per check, prefixed `[ok]`, `[--]` (no data) or `[FAIL]`. For example:
- `[ok] token arm`
- `[ok] discovery: 7 instances`
- `[ok] requests/failed total=24 points=48`
- `[--] browserTimings/totalDuration no data`
- `[ok] summary users_5m=165 sessions=360 servers=2 p95=480ms`
- `[ok] users/sessions series: 48 rows`
- `[FAIL] average metrics: HTTP 403: …`

It runs one `refresh_dashboard(range, View::Failures)` and reports per metric and per source. It never prints tokens, tenant IDs or subscription IDs. Instance lookup: match by name (case-insensitive) or resource ID. When `app` is None, it lists the instance names and checks nothing else. Several matches print the candidates (name, resource group, subscription name) and count as `[FAIL]`.

- [ ] **Step 1: Write failing tests:**
  - `probe_with_demo_backend_is_ok`: `StaticTokenSource` + `DemoBackend`, app `demo-app-prod`. The output contains `[ok] discovery: 2 instances` and the call returns `Ok(true)`.
  - `probe_reports_failures`: FakeBackend whose average metrics fail. The output contains `[FAIL] average metrics` and the call returns `Ok(false)`.
- [ ] **Step 2:** Run `cargo test probe`. Expected: FAIL.
- [ ] **Step 3:** Implement `run_probe` and the main dispatch. The real path uses `AzCliTokenSource::new()` and `AzureBackend::new(AzureHttp::new(..., Endpoints::default()))`, and `--demo` swaps in `DemoBackend`.
- [ ] **Step 4:** Run `cargo test`. Expected: PASS. Run `cargo run -- --demo probe --app demo-app-prod`. Expected: only `[ok]` lines.
- [ ] **Step 5:** Run the scrub check, then commit with `feat: probe diagnostics subcommand`.
- [ ] **Step 6 (USER CHECKPOINT):** Ask the user whether the Azure CLI is installed and `az login` is done on this machine. If not yet, continue with Task 10 and come back here before Task 14's live check.
- [ ] **Step 7:** Run `cargo run -- probe --app <instance name from the user>`. Expected: no `[FAIL]` lines.
- [ ] **Step 8:** Read the reference dashboard **outside the repo**: `az resource show --ids "$(cat .dev-dashboard-id)" -o json > <scratchpad>/dashboard.json`. For each metric chart tile, collect the metric names, namespaces and aggregations, and compare them with `catalog.rs` and the users/sessions KQL. Wherever a tile uses a different metric or aggregation for a v1 panel, change the catalog to match. Describe the change in the commit without any IDs.
- [ ] **Step 9:** Parity check. Set the portal dashboard to "Past 4 hours" and run `cargo run -- probe --app <name> --range 4h` within the same minute. Compare failed requests, server exceptions, dependency failures, server response avg, availability %, sessions, users, process CPU avg, available memory and process I/O.
  - Accept counts within 2% (ingestion lag) and averages within 1%.
  - If the portal shows memory in 1000-based units, switch `fmt_bytes` to base 1000.
  - Report the comparison to the user in chat. Do not commit numbers.
  - Commit any fixes with `fix(data): align metrics with portal dashboard`.

---

### Task 10: Theme and color modes

**Files:**
- Create: `src/ui/mod.rs` (module declarations only for now), `src/ui/theme.rs`

**Interfaces:**
- Consumes: `ColorMode` (Task 8).
- Produces:
```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub struct Rgb(pub u8, pub u8, pub u8);
pub mod palette { // exact values from the mockup
  BG 0f111a, BORDER 2f3654, TEXT c0caf5, SOFT a9b1d6, DIM 565f89, BLUE 7aa2f7, CYAN 7dcfff, TEAL 73daca, GREEN 9ece6a,
  YELLOW e0af68, ORANGE ff9e64, RED f7768e, PURPLE bb9af7, MAGENTA ff007c, SEL_BG 283457, METER_OFF 252a40, SKY 2ac3de }
#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum Gradient { Load, Cool, Hot, Violet, Teal, Lime, Ok }
impl Gradient { pub fn stops(self) -> &'static [Rgb] }
// Load GREEN→YELLOW→RED · Cool SKY→BLUE→PURPLE · Hot YELLOW→ORANGE→RED · Violet BLUE→PURPLE→MAGENTA
// Teal TEAL→CYAN→BLUE · Lime GREEN→TEAL→YELLOW · Ok GREEN→GREEN
#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum Depth { TrueColor, Ansi256 }
pub fn detect_depth(get: impl Fn(&str) -> Option<String>) -> Depth;
pub fn resolve_depth(mode: ColorMode, detected: Depth) -> Depth;
pub fn rgb_to_ansi256(c: Rgb) -> u8;   // nearest of xterm 6×6×6 cube (levels 0,95,135,175,215,255) and gray ramp 232–255 (8+10i), squared RGB distance
pub fn lerp(stops: &[Rgb], t: f64) -> Rgb;
#[derive(Clone, Copy, Debug)] pub struct Theme { pub depth: Depth }
impl Theme { pub fn new(depth: Depth) -> Self; pub fn color(&self, c: Rgb) -> ratatui::style::Color;
             pub fn grad(&self, g: Gradient, t: f64) -> Color; pub fn base(&self) -> Style /* fg TEXT, bg BG */ }
```
`detect_depth` rules, applied in order:
1. `COLORTERM` ∈ {truecolor, 24bit} → TrueColor.
2. `WT_SESSION` set → TrueColor.
3. `TERM_PROGRAM == "Apple_Terminal"` → Ansi256.
4. `TERM_PROGRAM` ∈ {iTerm.app, WezTerm, vscode, ghostty} → TrueColor.
5. `TERM` contains any of {kitty, alacritty, foot, ghostty, wezterm} → TrueColor.
6. Otherwise Ansi256.

- [ ] **Step 1: Write failing tests:**
  - `detect_depth`: `{COLORTERM=truecolor}` → TrueColor; `{TERM_PROGRAM=Apple_Terminal, TERM=xterm-256color}` → Ansi256; `{WT_SESSION=1}` → TrueColor; `{}` → Ansi256.
  - `rgb_to_ansi256`: (0,0,0) → 16, (255,255,255) → 231, (255,0,0) → 196, (128,128,128) → 244, (15,17,26) → 233.
  - `lerp(Load.stops(), 0.0) == GREEN`, `1.0 → RED`, `0.5 → YELLOW`.
  - `Theme::new(Ansi256).color(RED)` is `Color::Indexed(_)`, and the TrueColor version is `Color::Rgb(0xf7,0x76,0x8e)`.
  - `resolve_depth(ColorMode::Truecolor, Ansi256) == TrueColor`, and `Auto` returns the detected depth.
- [ ] **Step 2:** Run `cargo test ui::theme`. Expected: FAIL.
- [ ] **Step 3:** Implement the theme.
- [ ] **Step 4:** Run `cargo test ui::theme`. Expected: PASS.
- [ ] **Step 5:** Run the scrub check, then commit with `feat(ui): theme, gradients and color depth fallback`.

---

### Task 11: Widgets (braille chart, sparkline, big digits, meter, panel)

**Files:**
- Create: `src/ui/widgets/mod.rs`, `braille.rs`, `bigdigits.rs`, `meter.rs`, `panel.rs`

**Interfaces:**
- Consumes: `Theme`, `Gradient`, `palette`, `resample`, `truncate_width`.
- Produces:
```rust
// braille.rs
pub fn braille_rows(samples: &[f64], height: usize, mirrored: bool) -> Vec<String>; // samples 0..=1; 2 per char column (odd len → pad 0)
pub struct AreaChart<'a> { pub values: &'a [Option<f64>], pub max: Option<f64>, pub gradient: Gradient, pub mirrored: bool, pub theme: &'a Theme }
impl ratatui::widgets::Widget for AreaChart<'_>;
pub fn sparkline(values: &[f64], width: usize) -> String; // ▁▂▃▄▅▆▇█, resampled; all-zero → "▁"×width
// bigdigits.rs
pub fn big_text(s: &str) -> [String; 3];
// meter.rs
pub struct Meter<'a> { pub ratio: f64, pub gradient: Gradient, pub theme: &'a Theme }  impl Widget // one ■ per cell
// panel.rs
#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum Emphasis { Normal, Focused, Alert, AlertFocused }
pub struct PanelBlock<'a> { pub title: &'a str, pub hotkey: Option<char>, pub right: Option<ratatui::text::Line<'a>>,
    pub emphasis: Emphasis, pub stale: Option<String>, pub theme: &'a Theme }
impl PanelBlock<'_> { pub fn inner(area: Rect) -> Rect; pub fn render(&self, area: Rect, buf: &mut Buffer) }
pub fn superscript(c: char) -> char; // '0'..='9' → ⁰¹²³⁴⁵⁶⁷⁸⁹
```

Braille algorithm (this matches the mockup exactly):
- Character column `c` uses samples `2c` (left) and `2c+1` (right).
- Dot count `n = round(v * 4h)`, but `n = max(n, 1)` when `v > 0`, so a single failure is always visible.
- Bits from top to bottom: left `[0x01, 0x02, 0x04, 0x40]`, right `[0x08, 0x10, 0x20, 0x80]`.
- For row `r` (0 = top) and dot `d` (0 = top), `k = if mirrored { r*4 + d } else { (h-1-r)*4 + (3-d) }`. The dot is set when `k < n`. The character is `U+2800 + bits`.
- Row color: gradient at `t = if mirrored { r/(h-1) } else { (h-1-r)/(h-1) }` (use `t = 0.5` when `h == 1`), on background BG.

`AreaChart` resamples to `2 × width` samples, treats None as 0, and normalizes by `max` (default: 1.1 × the largest value, or 1.0 when all values are zero).

Big-digit font (exact):
- `0` `["█▀█","█ █","▀▀▀"]`, `1` `["▀█ "," █ ","▀▀▀"]`, `2` `["▀▀█","█▀▀","▀▀▀"]`, `3` `["▀▀█"," ▀█","▀▀▀"]`
- `4` `["█ █","▀▀█","  ▀"]`, `5` `["█▀▀","▀▀█","▀▀▀"]`, `6` `["█▀▀","█▀█","▀▀▀"]`, `7` `["▀▀█","  █","  ▀"]`
- `8` `["█▀█","█▀█","▀▀▀"]`, `9` `["█▀█","▀▀█","▀▀▀"]`, `.` `[" "," ","▀"]`, `-` `["   ","▀▀▀","   "]`, space `["  ","  ","  "]`
- Glyphs are joined by one space. Unknown characters render as a space.

Meter: lit cells use `grad(i/(n-1))`, unlit cells use METER_OFF.

Panel:
- Rounded border colored BORDER (Normal), BLUE (Focused) or RED (Alert and AlertFocused; AlertFocused is also bold).
- Title: ` ` + `superscript(hotkey)` in ORANGE + title in TEXT (RED when alerting) + ` `.
- `right` is drawn at the top right. `stale` appends ` · stale {age}` in YELLOW.
- The inner area is filled with BG.

- [ ] **Step 1: Write failing tests:**
```rust
assert_eq!(braille_rows(&[1.0, 1.0], 1, false), vec!["⣿"]);
assert_eq!(braille_rows(&[0.0, 0.0], 1, false), vec!["\u{2800}"]);
assert_eq!(braille_rows(&[0.25, 0.0], 1, false), vec!["⡀"]);
assert_eq!(braille_rows(&[0.25, 0.0], 1, true), vec!["⠁"]);
assert_eq!(braille_rows(&[0.5, 0.5], 2, false), vec!["\u{2800}", "⣿"]);
assert_eq!(braille_rows(&[0.01, 0.0], 4, false)[3], "⡀");   // tiny non-zero is still visible
assert_eq!(sparkline(&[0., 1.], 2), "▁█"); assert_eq!(sparkline(&[0., 0., 0.], 3), "▁▁▁");
assert_eq!(big_text("10"), ["▀█  █▀█".to_string(), " █  █ █".into(), "▀▀▀ ▀▀▀".into()]);
assert_eq!(big_text("1.5"), ["▀█   █▀▀".to_string(), " █   ▀▀█".into(), "▀▀▀ ▀ ▀▀▀".into()]);
assert_eq!(superscript('7'), '⁷');
```
  Render tests on a ratatui `Buffer`:
  - `Meter{ratio:0.3}` in 10×1: 3 lit cells, the rest have fg METER_OFF, and every symbol is `■`.
  - `PanelBlock` in 30×5 with hotkey '2', title "failed requests" and `Alert`: cell (0,0) is `╭` with fg RED, row 0 contains `²failed requests`, and `inner` equals the area shrunk by 1 on every side.
  - `AreaChart` with all-None values in 10×3: no panic, all braille blanks.
- [ ] **Step 2:** Run `cargo test ui::widgets`. Expected: FAIL.
- [ ] **Step 3:** Implement the widgets.
- [ ] **Step 4:** Run `cargo test ui::widgets`. Expected: PASS.
- [ ] **Step 5:** Run the scrub check, then commit with `feat(ui): braille charts, big digits, meters and panels`.

---

### Task 12: App state machine (dashboard)

**Files:**
- Create: `src/app.rs`

**Interfaces:**
- Consumes: `View`, `Store`, `evaluate`/`AlertState`, `Config`, `AppInstance`, `TimeRange`, `refresh_step`, `DashboardUpdate`, `AzError`.
- Produces (Tasks 15–18 extend `DataRequest`, `DataEvent` and `App`; the names here stay stable):
```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PanelId { Sessions, FailedRequests, ServerResponse, PageLoad, ExceptionsDeps, Cpu, Availability, BrowserExceptions, MemoryIo,
                   FailuresMain, Health, DependencyFailures }
pub fn panels(view: View) -> &'static [PanelId];
//   Overview: Sessions, FailedRequests, ServerResponse, PageLoad, ExceptionsDeps, Cpu, Availability, BrowserExceptions, MemoryIo (keys 1–9)
//   Failures: FailuresMain, Health, Sessions, ServerResponse, PageLoad, BrowserExceptions, Cpu, MemoryIo, DependencyFailures, Availability (keys 1–9, 0)
pub fn hotkey(view: View, p: PanelId) -> Option<char>;
#[derive(Clone, Debug)] pub enum DataRequest { Refresh { instance: AppInstance, range: TimeRange, view: View }, Discover }
#[derive(Debug)] pub enum DataEvent { Dashboard(DashboardUpdate), Instances(Result<Vec<AppInstance>, AzError>) }
#[derive(Clone, Debug)] pub enum Command { Fetch(DataRequest), SaveConfig(Config), Copy(String), OpenUrl(String), Quit }
pub struct App {
    pub config: Config, pub instance: Option<AppInstance>, pub view: View, pub focus: Option<PanelId>,
    pub range: TimeRange, pub refresh_secs: u64, pub store: Store, pub alerts: AlertState, pub help: bool,
    pub auth_problem: Option<AuthError>, pub toast: Option<(String, Instant)>, pub refreshing: bool, pub next_refresh: Option<Instant>,
}
impl App {
    pub fn new(config: Config, instance: Option<AppInstance>, range: TimeRange, refresh_secs: u64) -> Self;
    pub fn on_key(&mut self, key: crossterm::event::KeyEvent, now: Instant) -> Vec<Command>;
    pub fn on_tick(&mut self, now: Instant) -> Vec<Command>;
    pub fn on_data(&mut self, ev: DataEvent, now: Instant, wall: DateTime<Utc>) -> Vec<Command>;
    pub fn seconds_to_refresh(&self, now: Instant) -> Option<u64>;
    pub fn toast(&mut self, msg: impl Into<String>, now: Instant);
}
```

Behavior:
- **`on_tick`:**
  - If an instance is set, no refresh is running, and `next_refresh` is None or due: set `refreshing = true` and return `Fetch(Refresh)`.
  - Toasts older than 3 s are dropped.
- **`on_data(Dashboard)`:**
  - Apply the update to the store, then re-evaluate alerts.
  - Set `refreshing = false` and `next_refresh = now + refresh_secs`.
  - If any error is `AzError::Auth(e)`: set `auth_problem = Some(e)` and `next_refresh = now + 15 s`. Otherwise `auth_problem = None`.
  - Data events never change `view` or `focus`.
- **Keys:**
  - **Tab / BackTab:** switch view and clear focus. Switching to Failures while no refresh is running fetches immediately, so the top lists appear promptly.
  - **Digits:** focus the panel with that hotkey. A digit with no panel is ignored.
  - **Right / Down:** next panel in `panels(view)` order, wrapping. From no focus, go to the first panel.
  - **Left / Up:** previous panel, wrapping.
  - **Esc:** close help if it's open, else clear focus.
  - **`r`:** fetch now unless a refresh is already running.
  - **`+` / `-`:** `refresh_step` up or down (`+` makes the interval longer, as in btop). Set `config.refresh_secs` and `next_refresh = now + new`, and return `SaveConfig`.
  - **`t`:** `range.next()`, clear the store, reset alerts, `config.range = range`, return `SaveConfig`, and fetch now if idle (else set `next_refresh = now`).
  - **`?`:** toggle help.
  - **`q` / Ctrl+C:** `Quit`.
  - **Enter:** no-op until Task 16.

- [ ] **Step 1: Write failing tests** (`k(KeyCode)` helper, fixed `Instant` base):
  - `tab_toggles_view_and_clears_focus`.
  - `digit_focuses_panel`: in Overview, `2` → FailedRequests; in Failures, `0` → Availability; in Overview, `0` → None.
  - `arrows_cycle_focus`: Right from None → Sessions; Left from Sessions → MemoryIo.
  - `tick_starts_one_refresh` (Review Focus 4): the first tick returns 1 Fetch. A second tick while refreshing returns `[]`. After `on_data(Dashboard)`, a tick at +29 s returns `[]` and a tick at +30 s returns 1 Fetch.
  - `failed_refresh_keeps_data` (Review Focus 4): a good update followed by an errors-only update leaves the store series in place.
  - `plus_minus_step_refresh_and_save`: `+` from 30 → 60 returns `SaveConfig` with `refresh_secs == 60`. `-` at 15 stays at 15.
  - `t_cycles_range_clears_store_and_fetches`.
  - `q_and_ctrl_c_quit`.
  - `auth_error_sets_auth_problem`: an update with `(Scalars, AzError::Auth(NotLoggedIn(..)))` sets `auth_problem`, and the next clean update clears it.
  - `data_never_changes_view`: in Failures with focus Health, apply 3 updates; view and focus are unchanged.
  - `switch_to_failures_fetches_when_idle`.
- [ ] **Step 2:** Run `cargo test app`. Expected: FAIL.
- [ ] **Step 3:** Implement `src/app.rs`.
- [ ] **Step 4:** Run `cargo test app`. Expected: PASS.
- [ ] **Step 5:** Run the scrub check, then commit with `feat: app state machine for dashboard views and refresh scheduling`.

---

### Task 13: Dashboard rendering

**Files:**
- Create: `src/ui/layout.rs`, `src/ui/overview.rs`, `src/ui/failures.rs`
- Modify: `src/ui/mod.rs`

**Interfaces:**
- Consumes: `App`, `Theme`, the widgets, the format helpers, `Store`, `AlertState`, `PanelId`, `hotkey`, `DemoBackend`.
- Produces:
```rust
pub const MIN_WIDTH: u16 = 80; pub const MIN_HEIGHT: u16 = 24;
pub fn draw(f: &mut ratatui::Frame, app: &App, theme: &Theme, now: DateTime<FixedOffset>);
pub struct OverviewLayout { pub header: Rect, pub kpis: Vec<Rect>, pub kpi_strip: Option<Rect>, pub panels: Vec<Rect>, pub footer: Rect }
pub fn overview_layout(area: Rect) -> OverviewLayout;  // kpis: 7 tiles when wide, else empty and kpi_strip = Some
pub struct FailuresLayout { pub header: Rect, pub main: Rect, pub health: Rect, pub panels: Vec<Rect>, pub footer: Rect } // 8 panels
pub fn failures_layout(area: Rect) -> FailuresLayout;
#[cfg(test)] pub fn demo_app(view: View) -> App; // refresh_dashboard(DemoBackend) at 2026-11-04T13:06:42Z applied to a fresh App
```

**Layout:**
- Header and footer are one row each.
- **Overview, wide** (w ≥ 120 and h ≥ 34): a KPI row of height 6 with 7 equal tiles, and a 3×3 grid below.
- **Overview, otherwise:** a one-line KPI strip (`users 165 │ sessions 360 │ req/min 412 │ failed 12 │ exceptions 30 │ resp 119ms │ avail 100%`), and the grid uses 3 columns if w ≥ 120, else 2 columns (rows = ceil(9/cols)).
- **Failures, wide** (w ≥ 120): a top band at 40% of the body height (main 70% wide, health 30%), then two rows of 4 panels.
- **Failures, narrow:** main full width (height 12), health full width (height 11), then the rest in 2 columns.
- **Below 80×24:** a centered message `Terminal too small: need 80×24, have {w}×{h}`.

**Header (left to right):**
- `◆ {instance}` in BLUE.
- The label badge (`config.label_for`), shown as RED background, BG foreground, bold.
- `│`, then the tabs `overview` and `failures`. The active tab has a BLUE background, BG foreground and bold text.
- The failures tab while its alert is active: RED text with ` ● {failed_5m}` appended; when it is the active tab, a RED background. When it is not the active tab, the label alternates RED and DIM on even and odd seconds (the pulse).
- `│ range {label} │ refresh {s}s next {n}s ⟳`, or `refreshing…` while a refresh runs.
- `│ ● {servers} servers` in GREEN.
- Right-aligned: `HH:MM:SS` in CYAN, then ` {Wed 4 Nov} · UTC{+02:00}` in DIM. When there is a `last_error` and the data is older than 2 intervals, prefix `⚠ data {age} old` in YELLOW.

**Footer:**
- `tab switch view  1-9 select panel  ↵ drill in  esc back  r refresh  +/- interval  t time range  i instance  ? help  q quit`, with keys in ORANGE and text in DIM.
- A toast replaces the footer for 3 s.

**Help:** a centered overlay box with the key table from spec section 6.

**Auth problem:** a full-screen centered panel titled `Azure CLI sign-in needed`.
- It shows the message and `Run "az login" in another terminal. Retrying automatically.`
- For `AzNotInstalled`, the text is instead `Install the Azure CLI: https://aka.ms/azcli`.
- Never show stack traces.

**Overview panels:**

| Panel | Title (right) | Chart | Legend |
|---|---|---|---|
| Sessions | `sessions & users` (range) | Sessions up (Cool) + Users mirrored (Violet) | `▲ sessions {sessions_range}  ▼ users {users_range}` |
| FailedRequests | `failed requests` (`▲ {n} in 5m`, RED when alerting, else DIM `5m: {n}`) | RequestsFailed (Hot) | `■ failed {total} · {range}` |
| ServerResponse | `server response` (avg) | ServerResponse (Teal) | `■ avg {ms}  p95 {ms}` |
| PageLoad | `page load` (browser) | PageLoadTotal (Lime) | `client {processing}  network {network}  send {send}` |
| ExceptionsDeps | `exceptions / dependencies` | ExceptionsServer up (Violet) + DependenciesFailed mirrored (Hot) | `▲ exceptions {n}  ▼ dep. failures {n}` |
| Cpu | `cpu` (process) | ProcessCpu (Load), max 100 | `process {pct}%  processor {pct}%` |
| Availability | `availability` | Availability (Ok), max 100 | `avail {pct}%  tests {count}` |
| BrowserExceptions | `browser exceptions` | ExceptionsBrowser (Violet) | `count {n}` |
| MemoryIo | `memory & i/o` (available) | MemoryAvailable (Lime) | `available {bytes}  io {rate}` |

Panel rules:
- Mirrored pairs split the chart area 60/40 by rows. The chart height is the inner height − 1 (the legend line).
- A series without data shows a centered DIM `no data in range`, and the legend values show `—` (Review Focus 1).
- A stale series sets `PanelBlock.stale` to `fmt_age(age)`.
- Emphasis comes from focus plus alerts: FailedRequests and FailuresMain ← Failures; ExceptionsDeps and DependencyFailures ← ExceptionsDeps; Availability ← Availability; ServerResponse ← Response.

**KPI tiles** (big digits from `fmt_big`; the unit suffix goes on the label line):

| Tile | Value | Label | Color |
|---|---|---|---|
| users | users_5m | active · 5m | CYAN |
| sessions | sessions_range | unique · {range} | CYAN |
| requests | total(RequestsCount) / range.minutes() | per min · avg | BLUE |
| failed | failed_5m | last 5m | RED with an alert border when alerting, GREEN when 0 |
| exceptions | total(ExceptionsServer) | server · {range} | PURPLE |
| response | total(ServerResponse) | ms avg | YELLOW |
| availability | total(Availability) | % · {range} | GREEN; missing → `—` and `no tests` |

**Failures view:**
- **FailuresMain:**
  - Title `failures`. Right title `▲ {n} failed requests in last 5m` (RED), or `no failures in last 5m` (GREEN).
  - Left: RequestsFailed (Hot, 60% of rows) plus ExceptionsServer mirrored (Violet, 40%), and the legend `▲ failed requests {total}  ▼ server exceptions {total}  ■ dep. failures {total} · {range}`.
  - Right column, 38 wide: `top failing endpoints · 15m`, then up to 5 rows `{n:>3} {name}` (first row RED, the rest YELLOW); a blank line; `top exceptions · 15m` with up to 5 rows in PURPLE. Then `↵ drill in` in DIM.
  - Tops missing → `loading…`. Tops empty → `none` in GREEN.
- **Health:** each row is a 13-column label, a 14-cell meter and a right-aligned value:

  | Row | Meter | Ratio |
  |---|---|---|
  | availability | Ok | pct / 100 |
  | response avg | Load | min(avg / 2000, 1) |
  | response p95 | Load | min(p95 / 2000, 1) |
  | cpu | Load | cpu / 100 |
  | memory free | Lime | latest / max in range |
  | dep. failures 5m | Hot | min(n / threshold, 1) |
  | avail. tests | Ok | the availability ratio; value shows the count |

  After the meters come `sessions {n}  users {n}` and `requests {n}/min  servers {n}`.
- **Panels:** Sessions (titled `traffic`), ServerResponse, PageLoad, BrowserExceptions, Cpu, MemoryIo, DependencyFailures (`dependency failures`, DependenciesFailed in Hot, legend `failed {total}`), and Availability (`availability tests`, AvailabilityCount in Teal, legend `runs {count}  avail {pct}%`).

- [ ] **Step 1: Write failing tests** (TestBackend, `now = 2026-11-04T15:06:42+02:00`):
  - insta snapshots: `overview_160x45`, `overview_100x30`, `failures_160x45`, `failures_100x30`, `too_small_70x20`, `auth_required_100x30`, `help_100x30`.
  - `failed_panel_border_red_when_alert`: the FailedRequests panel's top-left cell has fg `theme.color(RED)`.
  - `focused_panel_border_blue`.
  - `failures_tab_label_red_when_alert`.
  - `no_data_panel_says_so` (Review Focus 1): with the PageLoad series removed from the store, the buffer contains `no data in range`.
  - `long_names_do_not_panic` (Review Focus 3): tops with a 300-character endpoint name and a CJK exception type, rendered at 80×24 and 160×45. No panic, and no text goes outside its panel.
  - `all_sizes_render`: for w in (20..=200).step_by(7) and h in (5..=60).step_by(5), drawing either view doesn't panic.
- [ ] **Step 2:** Run `cargo test ui::`. Expected: FAIL.
- [ ] **Step 3:** Implement the layout, header, footer, help, states, overview and failures.
- [ ] **Step 4:** Run `cargo test ui::`, review the snapshots against the mockup's structure, and run `cargo insta accept`. Expected: PASS.
- [ ] **Step 5:** Run the scrub check, then commit with `feat(ui): overview and failures dashboards`.

---

### Task 14: Runtime: terminal, event loop, data service

**Files:**
- Create: `src/run.rs`
- Modify: `src/main.rs`

**Interfaces:**
- Consumes: `Cli`, `config::{load, save, default_path}`, `App`, `draw`, `Theme`/`detect_depth`/`resolve_depth`, `Backend` impls, `AzureHttp`, `AzCliTokenSource`, `run_probe`.
- Produces:
```rust
pub fn spawn_data_service(backend: Arc<dyn Backend>, mut rx: mpsc::UnboundedReceiver<DataRequest>, tx: mpsc::UnboundedSender<DataEvent>) -> tokio::task::JoinHandle<()>;
// each request runs in its own spawned task; Refresh → refresh_dashboard(now = Utc::now()); Discover → instances()
pub fn accept_key(ev: &crossterm::event::KeyEvent) -> bool; // only KeyEventKind::Press (Windows also sends Release)
pub struct Effects { /* Option<arboard::Clipboard>, kept alive for the program's lifetime */ }
impl Effects { pub fn new() -> Self; pub fn copy(&mut self, text: &str) -> Result<(), String>; pub fn open(&self, url: &str) -> Result<(), String> }
pub async fn run(cli: Cli) -> Result<(), String>;
```

Startup:
- `path = cli.config` or `default_path()`, then `(config, warning) = load(path)`. A warning becomes a toast.
- Range and refresh come from the CLI if given, else from config. CLI values are not saved unless the user changes them with keys.
- Backend: `DemoBackend` with `--demo`, else `AzureBackend::new(AzureHttp::new(Arc::new(AzCliTokenSource::new()), Endpoints::default()))`.
- Instance: `config.last_instance`. With `--demo` and no saved instance, use the first demo instance. Task 15 adds `--app` and the picker.
- Theme: `resolve_depth(cli.color or config.color, detect_depth(|k| std::env::var(k).ok()))`.

Event loop:
- `let mut terminal = ratatui::init();`, which installs a panic hook that restores the terminal.
- `EventStream`, plus `tokio::time::interval(1s)` with `MissedTickBehavior::Skip`.
- `tokio::select!` over:
  - key events (filtered by `accept_key`) → `on_key`
  - Resize → redraw
  - the data channel → `on_data`
  - tick → `on_tick` and a redraw for the clock
- Commands:
  - `Fetch` → send to the service.
  - `SaveConfig` → `config::save` (an error becomes a toast).
  - `Copy` → `effects.copy`, then the toast `copied`.
  - `OpenUrl` → `effects.open`.
  - `Quit` → break.
- Draw only when something above happened, with `now = Local::now().fixed_offset()`. On exit, call `ratatui::restore()`.

`Effects::copy` tries arboard first. If that fails, it writes OSC 52 (`\x1b]52;c;{base64}\x07`) to stdout. `open` uses `open::that_detached`.

- [ ] **Step 1: Write failing tests:**
  - `data_service_answers_refresh`: DemoBackend; send `Refresh`; receive `DataEvent::Dashboard` with non-empty series within 2 s.
  - `data_service_runs_requests_concurrently`: FakeBackend whose `query` sleeps 300 ms; send `Refresh` then `Discover`. `Instances` arrives first.
  - `accept_key_ignores_release`.
- [ ] **Step 2:** Run `cargo test run`. Expected: FAIL.
- [ ] **Step 3:** Implement `run.rs` and wire `main.rs` (`#[tokio::main]`, dispatching `run` or probe).
- [ ] **Step 4:** Run `cargo test`. Expected: PASS.
- [ ] **Step 5 (manual):** Run `cargo run -- --demo`. Check that the Overview renders with demo data, Tab switches to Failures, the Failures tab pulses red, `+`/`-` change the interval, `t` changes the range, `?` toggles help, and `q` exits cleanly with the terminal restored.
- [ ] **Step 6:** Idle CPU check (anti-goal 2). Start `cargo run --release -- --demo` in another terminal and wait 60 s. Then `ps -o %cpu= -p "$(pgrep -f 'appi-dashboard --demo' | head -1)"`. Expected: < 1.0.
- [ ] **Step 7:** If Task 9's checkpoint is done, run `cargo run --release -- --app <name>`. Expected: real data, and the headline numbers match the probe output.
- [ ] **Step 8:** Run the scrub check, then commit with `feat: runtime event loop and data service`.

---

### Task 15: Instance picker and startup resolution

**Files:**
- Create: `src/ui/picker.rs`
- Modify: `src/app.rs`, `src/run.rs`, `src/ui/mod.rs`

**Interfaces:**
- Consumes: `DataRequest::Discover`, `DataEvent::Instances`, `Config::remember`, `demo_instances`.
- Produces:
```rust
pub struct PickerState { pub query: String, pub all: Option<Result<Vec<AppInstance>, String>>, pub selected: usize,
                         pub can_cancel: bool, pub pending_app: Option<String> }
impl PickerState { pub fn filtered(&self) -> Vec<&AppInstance> }
pub fn matches(inst: &AppInstance, query: &str) -> bool; // case-insensitive subsequence over "name resource_group subscription_name"
pub enum Resolution { One(AppInstance), Many(Vec<AppInstance>), None }
pub fn resolve_app(arg: &str, all: &[AppInstance]) -> Resolution; // starts with "/subscriptions/" → resource-id match, else name match; both case-insensitive
// App additions:
pub picker: Option<PickerState>,
impl App { pub fn open_picker(&mut self, can_cancel: bool, pending_app: Option<String>) -> Vec<Command>; /* → [Fetch(Discover)] */ }
```
- **Startup** (in `run.rs`):
  - With `--app X`: open the picker with `pending_app = Some(X)`, `can_cancel = false`, and Discover. On `Instances`: `One` selects it immediately (the picker closes); `Many` leaves the picker with query X filtered to those matches; `None` leaves the picker with query X and the toast `no instance named X`.
  - Without `--app`: use `last_instance` if set, else open the picker with `can_cancel = false`.
- **Selecting:** set the instance, `config.remember`, clear the store, close the picker, set `refreshing = true`, and return `[SaveConfig, Fetch(Refresh)]`.
- **Picker keys:**
  - Printable characters (including `q`) append to the query and reset the selection. Backspace deletes.
  - Up and Down move the selection. Enter selects. Ctrl+R discovers again.
  - Esc closes the picker when `can_cancel`, otherwise quits. Ctrl+C always quits.
  - On the dashboard, `i` opens the picker with `can_cancel = instance.is_some()`.
- **UI:**
  - A centered panel, 80% × 70%, titled `select application insights`.
  - Input line `› {query}▏`.
  - Rows: `{name}` in TEXT, then `{resource_group}  {subscription_name}` in DIM. The selected row has a SEL_BG background.
  - States: `discovering instances…` while loading; an error shows the message in RED plus `ctrl+r retry · esc quit`.

- [ ] **Step 1: Write failing tests:**
  - `resolve_by_name_case_insensitive`, `resolve_by_resource_id`.
  - `resolve_duplicate_names_is_many`: two instances named `demo-app-prod` in different subscriptions.
  - `resolve_unknown_is_none`.
  - `matches_subsequence`: `"dap"` matches `demo-app-prod`.
  - `typing_filters_and_enter_selects`: returns `SaveConfig` (with `last_instance` set) and `Fetch(Refresh)`.
  - `esc_without_instance_quits`, `esc_with_instance_closes`, `i_opens_picker_and_discovers`, `q_in_picker_types_not_quits`.
  - `pending_app_one_match_selects_without_picker`.
  - insta snapshot `picker_100x30` (demo instances).
- [ ] **Step 2:** Run `cargo test picker app`. Expected: FAIL.
- [ ] **Step 3:** Implement the picker and the startup flow.
- [ ] **Step 4:** Run `cargo test`. Expected: PASS. Manual: `cargo run -- --demo --app demo` opens the picker filtered to both demo instances, and Enter selects one.
- [ ] **Step 5:** Run the scrub check, then commit with `feat: instance picker, --app resolution and remembered instance`.

---

### Task 16: Drill-down framework and failing endpoints

**Files:**
- Create: `src/data/drill.rs`, `src/ui/drill.rs`
- Modify: `src/app.rs`, `src/run.rs`, `src/data/demo.rs`, `src/ui/mod.rs`

**Interfaces:**
- Consumes: `Backend`, `kql::failing_endpoints`, `Table`, `sparkline`, `PanelBlock`.
- Produces:
```rust
#[derive(Clone, Debug, PartialEq)]
pub enum DrillTarget { FailingEndpoints, EndpointFailures { name: String },
    RequestDetail { operation_id: String, request_id: Option<String> },
    ExceptionTypes { browser: bool }, ExceptionOccurrences { browser: bool, exception_type: String },
    FailingDependencies, DependencyCalls { dep_type: String, target: String },
    SlowEndpoints, SlowRequests { name: String }, SlowPages,
    AvailabilityTests, AvailabilityFailures { test: String, location: String },
    InstanceBreakdown { metric: InstanceMetric } }
#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum InstanceMetric { Cpu, Memory }
impl DrillTarget { pub fn crumb(&self) -> String } // FailingEndpoints → "failed requests", EndpointFailures → name, RequestDetail → "request", …
#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum NumFmt { Count, Ms, Pct, Bytes }
#[derive(Clone, Debug, PartialEq)] pub enum Cell { Text(String), Num(f64, NumFmt), Time(DateTime<Utc>), Codes(Vec<(String, f64)>), Trend(Vec<f64>) }
#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum Severity { Normal, Warn, Bad }
#[derive(Clone, Debug, PartialEq)] pub struct ListRow { pub cells: Vec<Cell>, pub next: Option<DrillTarget>, pub severity: Severity }
#[derive(Clone, Debug, PartialEq)] pub struct ListData { pub title: String, pub right: String, pub columns: Vec<&'static str>, pub rows: Vec<ListRow> }
#[derive(Clone, Debug, PartialEq)] pub enum ScreenData { List(ListData) } // Task 17 adds Endpoint, Task 18 adds Request
pub async fn load(backend: &dyn Backend, inst: &AppInstance, range: TimeRange, target: &DrillTarget, now: DateTime<Utc>) -> Result<ScreenData, AzError>;
pub fn failing_endpoints_list(t: &Table, range: TimeRange) -> ListData;
// app.rs additions
pub fn drill_target(p: PanelId) -> Option<DrillTarget>; // this task: FailedRequests, FailuresMain → FailingEndpoints; all others None (Task 19 extends)
pub enum FrameData { Loading, Ready(ScreenData), Failed(String) }
pub struct DrillFrame { pub id: u64, pub target: DrillTarget, pub data: FrameData, pub selected: [usize; 3], pub sort: Option<(usize, bool)>, pub sub: usize }
pub drill: Vec<DrillFrame>,
// DataRequest gains: Drill { frame_id: u64, instance: AppInstance, range: TimeRange, target: DrillTarget }
// DataEvent gains:   Drill { frame_id: u64, result: Result<ScreenData, AzError> }
```
- **`failing_endpoints_list`:** group rows by `name`.
  - fails = sum of n; codes = sums per `resultCode`, sorted descending; trend = per-bin sums in time order.
  - Rows are sorted by fails descending. Severity is `Bad` when any code ≥ 500, else `Warn`. `next = EndpointFailures{name}`.
  - Columns `["endpoint","fails","result codes","trend"]`; `right = "last {range} · {total} failed"`.
  - `load` runs it with `timespan = range.iso()`. Targets not implemented yet return `Err(AzError::Decode("not implemented"))`; this is unreachable because `drill_target` doesn't produce them yet.
- **Keys:**
  - On the dashboard, Enter on a focused panel that has a `drill_target`: push a frame (`Loading`, new id) and return `Fetch(Drill)`.
  - **In a drill frame:**
    - Up, Down, PageUp, PageDown, Home and End move `selected[sub]`, clamped to the list.
    - Enter on a row with `next` pushes a new frame.
    - Esc pops a frame. When the stack is empty, the dashboard is back with its focus kept.
    - `s` cycles the sort column: numbers and times sort descending, text ascending.
    - `r` reloads the top frame.
    - Tab clears the drill stack and switches the view.
- **Results:** `on_data(Drill)` matches by frame id. A result for a frame that was already popped is ignored. Dashboard refresh keeps running underneath while a drill is open.
- **UI:**
  - A breadcrumb line: `{view name} › {crumbs joined by " › "}` in BLUE, with the separators in DIM.
  - The list sits in a `PanelBlock` (title, right). The column header is DIM; the selected row has SEL_BG and `▶`.
  - `Codes` cells render as `500×9 502×3` (5xx RED, 4xx YELLOW). `Trend` cells are a 10-wide sparkline colored by severity.
  - States: `loading…`, or for `Failed` the message in RED plus `r to retry`.
  - Footer: `↑↓ select  ↵ open  s sort  r reload  esc back  tab switch view`.
- **Demo:** `DemoBackend` answers `failing_endpoints` with made-up rows.

- [ ] **Step 1: Write failing tests:**
  - `failing_endpoints_list_groups_and_sorts` (fixture): `rows[0]` is `POST /api/orders` with fails 12, codes `[("500",9.),("502",3.)]` and trend length = number of distinct bins; `next == EndpointFailures`.
  - `enter_on_failed_panel_pushes_loading_frame_and_fetches`.
  - `esc_pops_back_to_dashboard_keeping_focus`.
  - `stale_drill_result_ignored`: push frame 1, pop it, then `on_data(Drill{frame_id:1})` doesn't panic and the stack stays empty.
  - `sort_cycles_columns`.
  - insta snapshot `failing_endpoints_100x30` (demo).
  - `long_endpoint_names_truncate` (Review Focus 3): a 300-character name at 80×24, no panic.
- [ ] **Step 2:** Run `cargo test drill app`. Expected: FAIL.
- [ ] **Step 3:** Implement the framework, the list rendering and the demo data.
- [ ] **Step 4:** Run `cargo test`. Expected: PASS. Manual: in `--demo`, press 2 then Enter to see the list, Esc to go back.
- [ ] **Step 5:** Run the scrub check, then commit with `feat: drill-down framework and failing endpoints list`.

---

### Task 17: Endpoint failures screen

**Files:**
- Modify: `src/data/drill.rs`, `src/app.rs`, `src/ui/drill.rs`, `src/data/demo.rs`

**Interfaces:**
- Consumes: `kql::{endpoint_exceptions, endpoint_dependencies, endpoint_recent_failures}`, `TopItem`.
- Produces:
```rust
#[derive(Clone, Debug, PartialEq)] pub struct DepItem { pub dep_type: String, pub target: String, pub n: f64 }
#[derive(Clone, Debug, PartialEq)] pub struct RecentFailure { pub at: DateTime<Utc>, pub code: String, pub duration_ms: f64, pub instance: String,
    pub operation_id: String, pub request_id: String, pub exception_type: Option<String> }
#[derive(Clone, Debug, PartialEq)] pub struct EndpointData { pub name: String, pub exceptions: Vec<TopItem>, pub dependencies: Vec<DepItem>, pub recent: Vec<RecentFailure> }
pub fn endpoint_data(name: &str, exceptions: &Table, deps: &Table, recent: &Table) -> EndpointData;
// ScreenData gains: Endpoint(EndpointData)
```
- **`load(EndpointFailures{name})`** runs the three queries concurrently with `timespan = range.iso()`.
- **Sub-panels:** sub 0 is exception types, sub 1 is failing dependencies, and sub 2 is recent failures. The frame opens on sub 2.
- **Keys:** Left and Right switch sub-panels. Enter on sub 2 opens `RequestDetail{operation_id, request_id: Some(id)}`. Enter on subs 0 and 1 is a no-op until Task 19.
- **Layout** (mockup step 3):
  - Top row, two panels side by side: `exception types` (type in PURPLE, message in DIM, a bar of █ scaled to the max n with width 14, then the count) and `failing dependencies` (type in DIM, target, bar in ORANGE, count).
  - Bottom panel: `recent failures` with columns `time code duration instance exception`. Times are local `HH:MM:SS`; 5xx codes are RED and 4xx YELLOW.
  - An empty section shows `none`. The focused sub-panel has a BLUE border.

- [ ] **Step 1: Write failing tests:**
  - `endpoint_data_parses_tables` (fixtures), including a missing `exType` → None.
  - `left_right_switch_sub_panels`.
  - `enter_on_recent_opens_request_detail`.
  - insta snapshot `endpoint_screen_120x34`.
- [ ] **Step 2:** Run `cargo test drill`. Expected: FAIL.
- [ ] **Step 3:** Implement the endpoint screen.
- [ ] **Step 4:** Run `cargo test`. Expected: PASS.
- [ ] **Step 5:** Run the scrub check, then commit with `feat: endpoint failures drill-down screen`.

---

### Task 18: Request detail, transaction, copy and portal link

**Files:**
- Modify: `src/data/drill.rs`, `src/app.rs`, `src/ui/drill.rs`, `src/data/demo.rs`

**Interfaces:**
- Consumes: `kql::{request_row, request_exceptions, transaction}`.
- Produces:
```rust
#[derive(Clone, Debug, PartialEq)] pub struct RequestInfo { pub at: DateTime<Utc>, pub name: String, pub code: String, pub duration_ms: f64, pub success: Option<bool>,
    pub instance: String, pub operation_id: String, pub request_id: String, pub session_id: String, pub user_id: String }
#[derive(Clone, Debug, PartialEq)] pub struct ExceptionInfo { pub exception_type: String, pub message: String, pub frames: Vec<String> }
#[derive(Clone, Debug, PartialEq)] pub struct TxRow { pub at: DateTime<Utc>, pub item_type: String, pub name: String, pub dep_type: String, pub target: String,
    pub duration_ms: f64, pub success: Option<bool>, pub code: String, pub id: String, pub parent_id: String }
#[derive(Clone, Debug, PartialEq)] pub struct TxNode { pub depth: usize, pub label: String, pub offset_ms: f64, pub duration_ms: f64, pub success: Option<bool>, pub code: String }
#[derive(Clone, Debug, PartialEq)] pub struct RequestData { pub request: Option<RequestInfo>, pub exception: Option<ExceptionInfo>, pub transaction: Vec<TxNode> }
pub fn build_tree(rows: Vec<TxRow>) -> Vec<TxNode>;
pub fn stack_frames(details: &serde_json::Value, max: usize) -> Vec<String>;
pub fn parse_success(s: &str) -> Option<bool>; // True/true/1 → Some(true); False/false/0 → Some(false); else None
pub fn portal_logs_link(tenant_id: &str, resource_id: &str, kql: &str, timespan: &str) -> String;
pub fn portal_resource_link(tenant_id: &str, resource_id: &str) -> String;
// ScreenData gains: Request(RequestData)
```
- **`build_tree`:**
  - Roots are rows whose `parent_id` matches no row's `id`. Children are ordered by `at`, and the output is a depth-first pre-order walk.
  - `offset_ms` = `at` − earliest `at`.
  - Labels: a request uses its name; a dependency uses `"{dep_type} {target} {name}"` trimmed.
  - A visited set guards against cycles, so each row appears exactly once.
- **`stack_frames`:**
  - `details` is an array, or a string containing a JSON array. Use the first element that has a non-empty `parsedStack`.
  - Each frame becomes `at {method}  {fileName}:{line}`. Omit `  {fileName}:{line}` when there is no file name, and `:{line}` when the line is 0 or missing.
  - Otherwise fall back to `rawStack`: its first `max` non-empty trimmed lines. Otherwise `[]`.
- **`portal_logs_link`:** `https://portal.azure.com/#@{tenant}/blade/Microsoft_Azure_Monitoring_Logs/LogsBlade/resourceId/{pct(resource_id)}/source/LogsBlade.AnalyticsShareLinkToQuery/q/{pct(base64(gzip(kql)))}/timespan/{timespan}`. `pct` percent-encodes every non-alphanumeric byte, including `/`.
- **`portal_resource_link`:** `https://portal.azure.com/#@{tenant}/resource{resource_id}/overview`.
- **Keys:**
  - On a request frame, `c` returns `Copy(operation_id)` (toast `copied operation_Id`).
  - `o` returns `OpenUrl(portal_logs_link(tenant, resource_id, kql::transaction(op), range.iso()))`.
  - On any other drill frame, `o` opens `portal_resource_link`.
- **Layout** (mockup step 4):
  - A `request` panel:
    - result code (5xx RED, 4xx YELLOW, 2xx GREEN), duration, instance, `operation_Id`, session and user;
    - a blank line, then the exception type in PURPLE and the message wrapped to the panel width (at most 3 lines);
    - frames with `at` in DIM.
  - An `end-to-end transaction` panel. Each row has:
    - indentation by depth with `├ ` and `└ ` prefixes, and the label truncated;
    - a bar from offset to offset + duration scaled into 24 cells (minimum `▏`), RED when the item failed, else GREEN;
    - the duration, then `✓` in GREEN or `✕` in RED.
  - Footer: `c copy operation_Id  o open in Azure portal  esc back`.

- [ ] **Step 1: Write failing tests:**
  - `build_tree_orders_depth_first`: R (id r1) at 0; d1 (parent r1) at +5 ms; d3 (parent d1) at +6 ms; d2 (parent r1) at +10 ms. Expect labels `[R, d1, d3, d2]`, depths `[0,1,2,1]` and offsets `[0,5,6,10]`.
  - `build_tree_handles_orphans_and_cycles`: a missing parent makes the row a root, and in an a↔b cycle each row appears once.
  - `stack_frames_from_parsed_stack` (made-up frame `OrderService.SaveAsync  OrderService.cs:142`), `stack_frames_from_json_string`, `stack_frames_from_raw_stack`, and `stack_frames_respects_max`.
  - `parse_success_variants`.
  - `portal_logs_link_roundtrip`: take the segment after `/q/`, percent-decode, base64-decode and gunzip it; the result equals the KQL. There must be no raw `/` inside the resource ID segment.
  - `c_copies_operation_id`, `o_opens_logs_link`.
  - insta snapshot `request_screen_120x34`.
- [ ] **Step 2:** Run `cargo test drill`. Expected: FAIL.
- [ ] **Step 3:** Implement the request screen.
- [ ] **Step 4:** Run `cargo test`. Expected: PASS. Manual: in `--demo`, go from failing endpoints to an endpoint to a request; `c` shows the toast.
- [ ] **Step 5:** Run the scrub check, then commit with `feat: request detail with transaction timeline, copy and portal link`.
- [ ] **Step 6 (USER CHECKPOINT, light):** With real data, press `o` on a request and ask the user whether the portal opens Logs with the transaction query. If the link format doesn't work, change `o` to open `portal_resource_link` and copy the KQL instead (toast `KQL copied, paste into Logs`). Tell the user, then commit with `fix: portal link fallback`.

---

### Task 19: Remaining drill-downs

**Files:**
- Modify: `src/data/kql.rs`, `src/data/drill.rs`, `src/app.rs`, `src/data/demo.rs`

**Interfaces:**
- Consumes: the list framework (Task 16), `fetch_metrics` via `Backend::metrics`.
- Produces: KQL builders `exception_types(browser: bool)`, `exception_occurrences(browser: bool, t: &str)`, `failing_dependencies()`, `dependency_calls(dep_type: &str, target: &str)`, `slow_endpoints()`, `slow_requests(name: &str)`, `slow_pages()`, `availability_tests()`, `availability_failures(test: &str, location: &str)`, each returning `String`, plus one converter `fn <target>_list(t: &Table, range: TimeRange) -> ListData` per target. `drill_target` gains the full mapping: ExceptionsDeps → `ExceptionTypes{browser:false}`, BrowserExceptions → `ExceptionTypes{browser:true}`, DependencyFailures → FailingDependencies, ServerResponse → SlowEndpoints, PageLoad → SlowPages, Availability → AvailabilityTests, Cpu → `InstanceBreakdown{Cpu}`, MemoryIo → `InstanceBreakdown{Memory}`; Sessions and Health stay None. On the endpoint screen, Enter on sub 0 opens `ExceptionOccurrences{browser:false, type}`, and on sub 1 opens `DependencyCalls{type, target}`.

Shared rules for every query: failures are matched with `tostring(success) == "False"` (availability: `in ("0","False","false")`), counts use `sum(itemCount)`, and lists use `top 50`.

| Target | KQL essentials | Columns | Enter → |
|---|---|---|---|
| ExceptionTypes{browser} | `exceptions` where `client_Type` == / != `"Browser"`; summarize n, detail = take_any(outerMessage), per-bin trend, by type | type, count, message, trend | ExceptionOccurrences |
| ExceptionOccurrences | exceptions where type == T (+ browser filter); top 50 by timestamp desc; project timestamp, operation_Name, cloud_RoleInstance, outerMessage, operation_Id | time, operation, instance, message | server: RequestDetail{operation_id, None}; browser: — |
| FailingDependencies | failed dependencies; summarize n by type, target, resultCode | type, target, fails, result codes | DependencyCalls |
| DependencyCalls | failed dependencies where type == T and target == G; top 50 by timestamp desc | time, name, code, duration, operation | RequestDetail |
| SlowEndpoints | requests summarize n, avg = sum(duration*itemCount)/sum(itemCount), p95 = percentile(duration, 95) by name; top 50 by p95 | endpoint, count, avg, p95 | SlowRequests |
| SlowRequests | requests where name == X; top 50 by duration desc | time, code, duration, instance | RequestDetail |
| SlowPages | pageViews summarize n, avg, p95 of duration by name; top 50 by p95 | page, views, avg, p95 | — |
| AvailabilityTests | availabilityResults summarize runs, failed by name, location; pct = 100*(runs−failed)/runs; order by pct asc | test, location, runs, failed, availability | AvailabilityFailures |
| AvailabilityFailures | failed availabilityResults where name == T and location == L; top 50 by timestamp desc | time, duration, message | — |
| InstanceBreakdown{Cpu} | two metrics calls on `performanceCounters/processCpuPercentage` (Average and Maximum), `Interval::Full`, split by `cloud/roleInstance` | instance, avg %, max % | — |
| InstanceBreakdown{Memory} | two calls on `performanceCounters/memoryAvailableBytes` (Average and Minimum), Full, split | instance, avg free, min free | — |

- [ ] **Step 1: Write failing tests:**
  - One converter test per target (11): from a fixture table (or `MetricSeries` for the breakdowns), check the columns, the first row's cells and `next`.
  - One escaping test per string-taking builder.
  - `drill_target_mapping_covers_every_panel`.
  - `demo_backend_answers_every_target` (non-empty rows).
  - insta snapshot `slow_endpoints_100x30`.
- [ ] **Step 2:** Run `cargo test drill kql`. Expected: FAIL.
- [ ] **Step 3:** Implement the builders, converters, loaders, mapping and demo data.
- [ ] **Step 4:** Run `cargo test`. Expected: PASS. Manual: in `--demo`, every panel except sessions/traffic and health drills in, and Esc returns.
- [ ] **Step 5:** Run the scrub check, then commit with `feat: exceptions, dependencies, performance, availability and per-instance drill-downs`.

---

### Task 20: Release workflow, README, publish

**Files:**
- Create: `.github/workflows/release.yml`
- Modify: `README.md`, `Cargo.toml` (`description`, `repository = "https://github.com/suomiton/appi-dashboard"`, `license = "MIT OR Apache-2.0"`, `readme`, `keywords`, `categories`)

**`release.yml`:** runs on push of tags `v*`, with `permissions: contents: write`. Matrix:

| target | runner | archive |
|---|---|---|
| x86_64-unknown-linux-gnu | ubuntu-22.04 | tar.gz |
| aarch64-unknown-linux-gnu | ubuntu-22.04-arm | tar.gz |
| x86_64-apple-darwin | macos-latest (`rustup target add`) | tar.gz |
| aarch64-apple-darwin | macos-latest | tar.gz |
| x86_64-pc-windows-msvc | windows-latest | zip |

Each job runs `cargo build --release --locked --target $T` and packages `appi-dashboard-${{ github.ref_name }}-$T.{tar.gz|zip}` containing the binary, README and licenses, plus a `.sha256` file. Upload with `softprops/action-gh-release@v2`.

**README sections:**
- What it is.
- Requirements: Azure CLI, `az login`, and Reader on the App Insights resource.
- Install: GitHub Releases, or `cargo install --git https://github.com/suomiton/appi-dashboard`.
- Unsigned binaries: on macOS run `xattr -d com.apple.quarantine ./appi-dashboard`; on Windows SmartScreen choose "More info → Run anyway".
- Usage: the flags, `--demo` and `probe`.
- The key table from spec section 6.
- Config file locations per OS, with an example `config.toml` showing `refresh_secs`, `range`, `color`, `[alerts]` and `[labels]`.
- Data notes: data is 1–3 minutes behind; Azure bills metrics API calls beyond its free allowance, under your own Azure budget.
- Troubleshooting: not logged in, `az.cmd` on Windows, 256-color terminals.
- License.

- [ ] **Step 1:** Write `release.yml`, the README and the Cargo metadata. Run `cargo package --list` to check nothing private is included, and run `scripts/check-scrub.sh`.
- [ ] **Step 2:** Commit with `chore: release workflow and README`.
- [ ] **Step 3 (USER CHECKPOINT):** Confirm with the user before creating the public repository. Mention that commits carry their git author email, and that they can switch to their GitHub noreply address first if they prefer. Then run `gh auth status` (if it isn't authenticated, the user runs `! gh auth login`), followed by `gh repo create suomiton/appi-dashboard --public --source . --remote origin --push`.
- [ ] **Step 4:** Run `gh run list --limit 3`. Expected: the CI run succeeds on all 3 OSes. Fix any platform-specific failures and push again.
- [ ] **Step 5 (USER CHECKPOINT):** Ask before tagging. Then `git tag v0.1.0 && git push origin v0.1.0`. Run `gh release view v0.1.0`. Expected: 5 archives and 5 `.sha256` files.
- [ ] **Step 6 (USER CHECKPOINT):** The user tries the Windows build in Windows Terminal, first with `--demo` and then with a real instance. Collect any problems as follow-up fixes. Ask whether a colleague can try macOS; if not, record macOS as untested in the release notes.
