//! Deterministic fake data for `--demo` and for tests.
//!
//! Values depend only on the metric name and the bucket's timestamp, so the
//! same window always produces the same numbers. All names are made up.

use chrono::{DateTime, Duration, Utc};
use serde_json::{Value, json};

use crate::azure::discovery::AppInstance;
use crate::azure::http::AzError;
use crate::azure::logs::Table;
use crate::azure::metrics::{Interval, MetricSeries, MetricsRequest};
use crate::data::backend::Backend;
use crate::data::kql;
use crate::data::time::TimeRange;

const ZERO: &str = "00000000-0000-0000-0000-000000000000";
const GB: f64 = 1e9;
const MB: f64 = 1e6;

pub fn demo_instances() -> Vec<AppInstance> {
    ["demo-app-prod", "demo-app-staging"]
        .iter()
        .enumerate()
        .map(|(i, name)| AppInstance {
            name: name.to_string(),
            resource_id: format!(
                "/subscriptions/{ZERO}/resourceGroups/demo-rg/providers/microsoft.insights/components/{name}"
            ),
            resource_group: "demo-rg".into(),
            subscription_id: ZERO.into(),
            subscription_name: "Demo Subscription".into(),
            tenant_id: ZERO.into(),
            app_id: format!("{i}0000000-0000-0000-0000-000000000000"),
            location: "westeurope".into(),
        })
        .collect()
}

pub struct DemoBackend {
    seed: u64,
    /// When set, time-relative demo data is anchored here instead of the
    /// real clock, so tests and snapshots are reproducible.
    fixed_now: Option<DateTime<Utc>>,
}

impl Default for DemoBackend {
    fn default() -> Self {
        Self::new()
    }
}

/// A stable pseudo-random value in [0, 1) for (seed, key, i).
fn noise(seed: u64, key: &str, i: i64) -> f64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in key.bytes() {
        h = (h ^ b as u64).wrapping_mul(0x100000001b3);
    }
    let mut z = seed ^ h ^ (i as u64).wrapping_mul(0x9E3779B97F4A7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
    z ^= z >> 31;
    (z >> 11) as f64 / (1u64 << 53) as f64
}

fn step_minutes(interval: Interval, start: DateTime<Utc>, end: DateTime<Utc>) -> i64 {
    match interval {
        Interval::PT1M => 1,
        Interval::PT5M => 5,
        Interval::PT15M => 15,
        Interval::PT30M => 30,
        Interval::Full => (end - start).num_minutes().max(1),
    }
}

impl DemoBackend {
    pub fn new() -> Self {
        Self {
            seed: 0x5eed,
            fixed_now: None,
        }
    }

    pub fn at(now: DateTime<Utc>) -> Self {
        Self {
            fixed_now: Some(now),
            ..Self::new()
        }
    }

    /// One bucket of `metric` starting at `at`, `step` minutes long.
    fn bucket(
        &self,
        metric: &str,
        at: DateTime<Utc>,
        end: DateTime<Utc>,
        step: i64,
    ) -> Option<f64> {
        let i = at.timestamp() / 60 / step;
        let n = noise(self.seed, metric, i);
        let n2 = noise(self.seed ^ 1, metric, i);
        let to_end = (end - at).num_minutes();
        let recent = to_end <= 10;
        let s = step as f64;
        Some(match metric {
            "requests/count" => 412.0 * s * (0.85 + 0.3 * n),
            "requests/failed" => {
                if step == 1 && to_end <= 5 {
                    [2.0, 3.0, 2.0, 3.0, 2.0][((to_end - 1).clamp(0, 4)) as usize]
                } else if recent {
                    (2.0 * s).max(1.0)
                } else if n < 0.05 {
                    1.0
                } else {
                    0.0
                }
            }
            "exceptions/server" => {
                if recent {
                    3.0
                } else if n < 0.15 {
                    (n2 * 6.0).floor()
                } else {
                    0.0
                }
            }
            "exceptions/browser" => {
                if n < 0.2 {
                    1.0 + (n2 * 15.0).floor()
                } else {
                    (n2 * 3.0).floor()
                }
            }
            "dependencies/failed" => {
                if n < 0.25 {
                    1.0 + (n2 * 4.0).floor()
                } else {
                    0.0
                }
            }
            "availabilityResults/count" => s,
            "requests/duration" => 110.0 + 30.0 * n + if n2 < 0.05 { 900.0 } else { 0.0 },
            "browserTimings/totalDuration" => {
                560.0 + 300.0 * n + if n2 < 0.08 { 800.0 } else { 0.0 }
            }
            "browserTimings/networkDuration" => 120.0 + 60.0 * n,
            "browserTimings/processingDuration" => 300.0 + 80.0 * n,
            "browserTimings/sendDuration" => 60.0 + 40.0 * n,
            "browserTimings/receiveDuration" => 20.0 + 15.0 * n,
            "performanceCounters/processCpuPercentage" => {
                0.25 + 0.2 * n + if n2 < 0.07 { 3.0 } else { 0.0 }
            }
            "performanceCounters/memoryAvailableBytes" => {
                (11.7 + 0.25 * ((i as f64) / 7.0).sin() + 0.05 * n) * GB
            }
            "performanceCounters/processIOBytesPerSecond" => {
                12.9 * MB * (0.7 + 0.6 * n) + if n2 < 0.02 { 500.0 * MB } else { 0.0 }
            }
            "availabilityResults/availabilityPercentage" => 100.0,
            _ => return None,
        })
    }

    /// The whole-window value, matching the mockup's legend numbers.
    fn full(&self, metric: &str, minutes: i64) -> Option<f64> {
        let m = minutes as f64;
        Some(match metric {
            "requests/count" => 412.0 * m,
            "requests/failed" => 24.0,
            "exceptions/server" => 30.0,
            "exceptions/browser" => 60.0,
            "dependencies/failed" => 45.0,
            "availabilityResults/count" => m,
            "requests/duration" => 119.48,
            "browserTimings/totalDuration" => 565.0,
            "browserTimings/networkDuration" => 148.95,
            "browserTimings/processingDuration" => 335.5,
            "browserTimings/sendDuration" => 80.28,
            "browserTimings/receiveDuration" => 27.4,
            "performanceCounters/processCpuPercentage" => 0.3137,
            "performanceCounters/memoryAvailableBytes" => 11.72 * GB,
            "performanceCounters/processIOBytesPerSecond" => 12.9 * MB,
            "availabilityResults/availabilityPercentage" => 100.0,
            _ => return None,
        })
    }

    fn users_sessions(&self, range: TimeRange) -> Table {
        let g = range.grain_minutes();
        let now = self.fixed_now.unwrap_or_else(Utc::now);
        let end = now - Duration::seconds(now.timestamp() % (g * 60));
        let n = range.minutes() / g;
        let rows = (0..n)
            .map(|k| {
                let at = end - Duration::minutes((n - k) * g);
                let i = at.timestamp() / 60 / g;
                let users = 6.0 + (noise(self.seed, "users", i) * 12.0).floor();
                let sessions = users + (noise(self.seed, "sessions", i) * 3.0).floor();
                json!([at.to_rfc3339(), users, sessions])
            })
            .collect();
        table(&["timestamp", "users", "sessions"], rows)
    }
}

const DEMO_ENDPOINTS: [&str; 4] = [
    "POST /api/orders",
    "GET /api/reports/export",
    "POST /api/auth/token",
    "GET /api/sites/{id}/readings",
];

/// (operation ID, request ID, minutes ago) of the demo's failed requests.
const DEMO_REQUESTS: [(&str, &str, i64); 4] = [
    ("op-7f3a9c2e", "req-0001", 8),
    ("op-1b8e4c1d", "req-0002", 9),
    ("op-4c1d7f3a", "req-0003", 15),
    ("op-9a0b1c2d", "req-0004", 20),
];

/// (type, target, result code, failures) of the demo's failing dependencies.
const DEMO_DEPENDENCIES: [(&str, &str, &str, i64); 3] = [
    ("SQL", "demo-sql.database.windows.net | appdb", "-2", 9),
    ("HTTP", "demo-func.azurewebsites.net", "502", 3),
    ("Azure blob", "demostorage", "409", 1),
];

fn demo_exception_types(browser: bool) -> &'static [(&'static str, i64, &'static str)] {
    if browser {
        &[
            (
                "TypeError",
                41,
                "Cannot read properties of undefined (reading 'id')",
            ),
            ("ChunkLoadError", 12, "Loading chunk 42 failed."),
            (
                "ResizeObserverError",
                7,
                "ResizeObserver loop limit exceeded",
            ),
        ]
    } else {
        &[
            ("SqlException", 9, SQL_TIMEOUT),
            ("HttpRequestException", 3, BAD_GATEWAY),
            ("TaskCanceledException", 2, "A task was canceled."),
        ]
    }
}

const SQL_TIMEOUT: &str = "Execution Timeout Expired. The timeout period elapsed prior to completion of the operation or the server is not responding.";
const BAD_GATEWAY: &str = "Response status code does not indicate success: 502 (Bad Gateway).";

impl DemoBackend {
    fn now(&self) -> DateTime<Utc> {
        self.fixed_now.unwrap_or_else(Utc::now)
    }

    /// Answers the drill-down list queries added for the other panels.
    fn drill_query(&self, q: &str, timespan: &str) -> Option<Table> {
        let range = TimeRange::ALL
            .into_iter()
            .find(|r| r.iso() == timespan)
            .unwrap_or_default();
        for browser in [false, true] {
            if TimeRange::ALL
                .iter()
                .any(|r| q == kql::exception_types(browser, *r))
            {
                return Some(self.exception_types(browser, range));
            }
            for (ty, _, _) in demo_exception_types(browser) {
                if q == kql::exception_occurrences(browser, ty) {
                    return Some(self.exception_occurrences(browser, ty));
                }
            }
        }
        if q == kql::failing_dependencies() {
            return Some(table(
                &["type", "target", "resultCode", "n"],
                DEMO_DEPENDENCIES
                    .iter()
                    .map(|(ty, target, code, n)| json!([ty, target, code, n]))
                    .collect(),
            ));
        }
        for (ty, target, code, _) in DEMO_DEPENDENCIES {
            if q == kql::dependency_calls(ty, target) {
                let rows = DEMO_REQUESTS
                    .iter()
                    .take(3)
                    .map(|(op, _, mins)| {
                        json!([
                            (self.now() - Duration::minutes(*mins)).to_rfc3339(),
                            "INSERT orders",
                            code,
                            29950.0,
                            op,
                            DEMO_ENDPOINTS[0]
                        ])
                    })
                    .collect();
                return Some(table(
                    &[
                        "timestamp",
                        "name",
                        "resultCode",
                        "duration",
                        "operation_Id",
                        "operation_Name",
                    ],
                    rows,
                ));
            }
        }
        let latency = |names: &[(&str, f64, f64, f64)]| {
            table(
                &["name", "n", "avg", "p95"],
                names
                    .iter()
                    .map(|(name, n, avg, p95)| json!([name, n, avg, p95]))
                    .collect(),
            )
        };
        if q == kql::slow_endpoints() {
            return Some(latency(&[
                ("GET /api/reports/export", 312.0, 2300.0, 8100.0),
                ("POST /api/orders", 1840.0, 420.0, 1900.0),
                ("GET /api/sites/{id}/readings", 22040.0, 95.0, 310.0),
                ("POST /api/auth/token", 5120.0, 60.0, 140.0),
            ]));
        }
        if q == kql::slow_pages() {
            return Some(latency(&[
                ("Reports", 420.0, 1800.0, 4200.0),
                ("Dashboard", 3100.0, 560.0, 1300.0),
                ("Sites", 1500.0, 410.0, 900.0),
            ]));
        }
        for name in DEMO_ENDPOINTS {
            if q == kql::slow_requests(name) {
                let rows = DEMO_REQUESTS
                    .iter()
                    .map(|(op, req, mins)| {
                        json!([
                            (self.now() - Duration::minutes(*mins * 3)).to_rfc3339(),
                            "200",
                            8100.0 - *mins as f64 * 100.0,
                            "demo-host-0",
                            op,
                            req
                        ])
                    })
                    .collect();
                return Some(table(
                    &[
                        "timestamp",
                        "resultCode",
                        "duration",
                        "cloud_RoleInstance",
                        "operation_Id",
                        "id",
                    ],
                    rows,
                ));
            }
        }
        if q == kql::availability_tests() {
            return Some(table(
                &["name", "location", "runs", "failed", "pct"],
                vec![
                    json!(["home page", "West Europe", 48, 1, 97.9]),
                    json!(["home page", "North Europe", 48, 0, 100.0]),
                    json!(["home page", "UK South", 48, 0, 100.0]),
                ],
            ));
        }
        if q == kql::availability_failures("home page", "West Europe") {
            return Some(table(
                &["timestamp", "duration", "message"],
                vec![json!([
                    (self.now() - Duration::minutes(95)).to_rfc3339(),
                    30000.0,
                    "Request timed out after 30 seconds."
                ])],
            ));
        }
        let _ = range;
        None
    }

    fn exception_types(&self, browser: bool, range: TimeRange) -> Table {
        let bins = self.bins(range);
        let n = bins.len();
        let mut rows = Vec::new();
        for (k, (ty, total, msg)) in demo_exception_types(browser).iter().enumerate() {
            let per = (*total as usize).max(1);
            for j in 0..per {
                let bin = bins[(n - 1).saturating_sub((j * 7 + k * 3) % n.max(1))].to_rfc3339();
                rows.push(json!([ty, msg, bin, 1]));
            }
        }
        table(&["type", "detail", "timestamp", "n"], rows)
    }

    fn exception_occurrences(&self, browser: bool, ty: &str) -> Table {
        let msg = demo_exception_types(browser)
            .iter()
            .find(|(t, _, _)| *t == ty)
            .map_or("", |(_, _, m)| *m);
        let rows = DEMO_REQUESTS
            .iter()
            .take(3)
            .map(|(op, _, mins)| {
                let op = if browser { "" } else { *op };
                let operation = if browser { "" } else { DEMO_ENDPOINTS[0] };
                json!([
                    (self.now() - Duration::minutes(*mins)).to_rfc3339(),
                    operation,
                    "demo-host-1",
                    msg,
                    op
                ])
            })
            .collect();
        table(
            &[
                "timestamp",
                "operation_Name",
                "cloud_RoleInstance",
                "outerMessage",
                "operation_Id",
            ],
            rows,
        )
    }

    fn request_start(&self, op: &str) -> DateTime<Utc> {
        let mins = DEMO_REQUESTS
            .iter()
            .find(|(o, _, _)| *o == op)
            .map_or(10, |(_, _, m)| *m);
        self.now() - Duration::minutes(mins) - Duration::seconds(30)
    }

    fn request_row(&self, op: &str) -> Table {
        let (req, code, duration) = match op {
            "op-4c1d7f3a" => ("req-0003", "502", 2100.0),
            "op-9a0b1c2d" => ("req-0004", "504", 60000.0),
            "op-1b8e4c1d" => ("req-0002", "500", 30010.0),
            _ => ("req-0001", "500", 30020.0),
        };
        table(
            &[
                "timestamp",
                "name",
                "resultCode",
                "duration",
                "success",
                "cloud_RoleInstance",
                "operation_Id",
                "id",
                "session_Id",
                "user_Id",
            ],
            vec![json!([
                self.request_start(op).to_rfc3339(),
                DEMO_ENDPOINTS[0],
                code,
                duration,
                "False",
                "demo-host-1",
                op,
                req,
                "1b8e0c2f4a6d",
                "4c1d9e8f7a6b"
            ])],
        )
    }

    fn request_exceptions(&self, op: &str) -> Table {
        let (ty, msg) = if op == "op-4c1d7f3a" {
            ("HttpRequestException", BAD_GATEWAY)
        } else {
            ("SqlException", SQL_TIMEOUT)
        };
        let details = json!([{"parsedStack": [
            {"level": 0, "method": "Demo.Orders.OrderService.SaveBatchAsync", "fileName": "OrderService.cs", "line": 142},
            {"level": 1, "method": "Demo.Orders.OrdersController.Create", "fileName": "OrdersController.cs", "line": 58}
        ]}]);
        table(
            &[
                "timestamp",
                "type",
                "outerMessage",
                "innermostMessage",
                "details",
            ],
            vec![json!([
                self.request_start(op).to_rfc3339(),
                ty,
                msg,
                msg,
                details
            ])],
        )
    }

    fn transaction(&self, op: &str) -> Table {
        let t0 = self.request_start(op);
        let at = |ms: i64| (t0 + Duration::milliseconds(ms)).to_rfc3339();
        table(
            &[
                "timestamp",
                "itemType",
                "name",
                "type",
                "target",
                "duration",
                "success",
                "resultCode",
                "id",
                "operation_ParentId",
            ],
            vec![
                json!([
                    at(0),
                    "request",
                    DEMO_ENDPOINTS[0],
                    "",
                    "",
                    30020.0,
                    "False",
                    "500",
                    "req-0001",
                    op
                ]),
                json!([
                    at(15),
                    "dependency",
                    "INSERT orders",
                    "SQL",
                    "appdb",
                    29950.0,
                    "False",
                    "-2",
                    "dep-1",
                    "req-0001"
                ]),
                json!([
                    at(29980),
                    "dependency",
                    "PUT uploads/batch.json",
                    "Azure blob",
                    "demostorage",
                    60.0,
                    "True",
                    "201",
                    "dep-2",
                    "req-0001"
                ]),
            ],
        )
    }

    fn endpoint_exceptions(&self, name: &str) -> Table {
        let rows = if name == DEMO_ENDPOINTS[0] {
            vec![
                json!(["SqlException", 9, SQL_TIMEOUT]),
                json!(["HttpRequestException", 3, BAD_GATEWAY]),
            ]
        } else {
            vec![json!(["TaskCanceledException", 2, "A task was canceled."])]
        };
        table(&["type", "n", "detail"], rows)
    }

    fn endpoint_dependencies(&self, name: &str) -> Table {
        let rows = if name == DEMO_ENDPOINTS[0] {
            vec![
                json!(["SQL", "demo-sql.database.windows.net | appdb", 9]),
                json!(["HTTP", "demo-func.azurewebsites.net", 3]),
            ]
        } else {
            vec![]
        };
        table(&["type", "target", "n"], rows)
    }

    fn endpoint_recent(&self, name: &str) -> Table {
        let at = |mins: i64, secs: i64| {
            (self.now() - Duration::minutes(mins) - Duration::seconds(secs)).to_rfc3339()
        };
        let rows = if name == DEMO_ENDPOINTS[0] {
            vec![
                json!([
                    at(8, 30),
                    "500",
                    30020.0,
                    "demo-host-1",
                    "op-7f3a9c2e",
                    "req-0001",
                    "SqlException"
                ]),
                json!([
                    at(9, 2),
                    "500",
                    30010.0,
                    "demo-host-0",
                    "op-1b8e4c1d",
                    "req-0002",
                    "SqlException"
                ]),
                json!([
                    at(15, 39),
                    "502",
                    2100.0,
                    "demo-host-1",
                    "op-4c1d7f3a",
                    "req-0003",
                    "HttpRequestException"
                ]),
            ]
        } else {
            vec![json!([
                at(20, 0),
                "504",
                60000.0,
                "demo-host-0",
                "op-9a0b1c2d",
                "req-0004",
                "TaskCanceledException"
            ])]
        };
        table(
            &[
                "timestamp",
                "resultCode",
                "duration",
                "cloud_RoleInstance",
                "operation_Id",
                "id",
                "exType",
            ],
            rows,
        )
    }

    fn bins(&self, range: TimeRange) -> Vec<DateTime<Utc>> {
        let g = range.grain_minutes();
        let now = self.fixed_now.unwrap_or_else(Utc::now);
        let end = now - Duration::seconds(now.timestamp() % (g * 60));
        let n = range.minutes() / g;
        (0..n)
            .map(|k| end - Duration::minutes((n - k) * g))
            .collect()
    }

    /// 24 failures over four made-up endpoints, the mockup's numbers.
    fn failing_endpoints(&self, range: TimeRange) -> Table {
        let bins = self.bins(range);
        let n = bins.len();
        let at = |i: usize| bins[i.min(n - 1)].to_rfc3339();
        let mut rows = vec![
            json!(["POST /api/orders", "500", at(n - 1), 4]),
            json!(["POST /api/orders", "500", at(n - 2), 5]),
            json!(["POST /api/orders", "502", at(n - 1), 3]),
            json!(["POST /api/auth/token", "401", at(n - 1), 3]),
            json!(["GET /api/sites/{id}/readings", "500", at(0), 2]),
        ];
        for k in 1..=7 {
            rows.push(json!(["GET /api/reports/export", "504", at(k * n / 8), 1]));
        }
        table(&["name", "resultCode", "timestamp", "n"], rows)
    }
}

fn table(columns: &[&str], rows: Vec<Value>) -> Table {
    Table {
        columns: columns.iter().map(|c| c.to_string()).collect(),
        rows: rows
            .into_iter()
            .map(|r| r.as_array().cloned().unwrap_or_default())
            .collect(),
    }
}

#[async_trait::async_trait]
impl Backend for DemoBackend {
    async fn instances(&self) -> Result<Vec<AppInstance>, AzError> {
        Ok(demo_instances())
    }

    async fn metrics(&self, req: MetricsRequest) -> Result<Vec<MetricSeries>, AzError> {
        let step = step_minutes(req.interval, req.start, req.end);
        let dims: Vec<(Option<String>, f64)> = match &req.split_by {
            Some(_) => vec![
                (Some("demo-host-0".into()), 1.2),
                (Some("demo-host-1".into()), 0.8),
            ],
            None => vec![(None, 1.0)],
        };
        let mut out = Vec::new();
        for name in &req.metric_names {
            for (dimension, scale) in &dims {
                let points = if req.interval == Interval::Full {
                    vec![(req.start, self.full(name, step).map(|v| v * scale))]
                } else {
                    let mut pts = Vec::new();
                    let mut at = req.start;
                    while at < req.end {
                        pts.push((at, self.bucket(name, at, req.end, step).map(|v| v * scale)));
                        at += Duration::minutes(step);
                    }
                    pts
                };
                out.push(MetricSeries {
                    name: name.clone(),
                    dimension: dimension.clone(),
                    points,
                });
            }
        }
        Ok(out)
    }

    async fn query(&self, _app_id: &str, kql_text: &str, timespan: &str) -> Result<Table, AzError> {
        if kql_text == kql::summary_scalars() {
            return Ok(table(
                &[
                    "users_5m",
                    "users_range",
                    "sessions_range",
                    "servers_10m",
                    "response_p95_ms",
                ],
                vec![json!([165, 343, 360, 2, 480.0])],
            ));
        }
        if kql_text == kql::failure_tops() {
            return Ok(table(
                &["section", "name", "detail", "n"],
                vec![
                    json!(["endpoint", "POST /api/orders", "", 12]),
                    json!(["endpoint", "GET /api/reports/export", "", 2]),
                    json!(["endpoint", "POST /api/auth/token", "", 1]),
                    json!([
                        "exception",
                        "SqlException",
                        "Execution Timeout Expired. The timeout period elapsed prior to completion of the operation.",
                        9
                    ]),
                    json!([
                        "exception",
                        "HttpRequestException",
                        "Response status code does not indicate success: 502 (Bad Gateway).",
                        3
                    ]),
                ],
            ));
        }
        if TimeRange::ALL
            .iter()
            .any(|r| kql_text == kql::failing_endpoints(*r))
        {
            let range = TimeRange::ALL
                .into_iter()
                .find(|r| r.iso() == timespan)
                .unwrap_or_default();
            return Ok(self.failing_endpoints(range));
        }
        if let Some(t) = self.drill_query(kql_text, timespan) {
            return Ok(t);
        }
        for (op, req, _) in DEMO_REQUESTS {
            if kql_text == kql::request_row(op, Some(req)) || kql_text == kql::request_row(op, None)
            {
                return Ok(self.request_row(op));
            }
            if kql_text == kql::request_exceptions(op) {
                return Ok(self.request_exceptions(op));
            }
            if kql_text == kql::transaction(op) {
                return Ok(self.transaction(op));
            }
        }
        for name in DEMO_ENDPOINTS {
            if kql_text == kql::endpoint_exceptions(name) {
                return Ok(self.endpoint_exceptions(name));
            }
            if kql_text == kql::endpoint_dependencies(name) {
                return Ok(self.endpoint_dependencies(name));
            }
            if kql_text == kql::endpoint_recent_failures(name) {
                return Ok(self.endpoint_recent(name));
            }
        }
        // Ranges sharing a grain produce identical KQL; the timespan decides.
        if let Some(range) = TimeRange::ALL
            .into_iter()
            .filter(|r| kql_text == kql::users_sessions_series(*r))
            .find(|r| r.iso() == timespan)
        {
            return Ok(self.users_sessions(range));
        }
        Ok(Table::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::alerts::{AlertConfig, evaluate};
    use crate::data::dashboard::refresh_dashboard;
    use crate::data::store::Store;
    use crate::data::time::TimeRange;

    #[tokio::test]
    async fn demo_backend_trips_failure_alert() {
        let b = DemoBackend::new();
        let inst = demo_instances().remove(0);
        let now = chrono::Utc::now();
        let u = refresh_dashboard(&b, &inst, TimeRange::H4, now).await;
        assert!(u.errors.is_empty());
        let mut s = Store::default();
        s.apply(u, now);
        assert!(evaluate(&AlertConfig::default(), &s, now, 30).failures());
        assert!(s.has_data(crate::data::catalog::SeriesId::Sessions));
        assert!(!s.has_data(crate::data::catalog::SeriesId::ProcessorCpu));
        assert_eq!(
            s.tops.as_ref().unwrap().value.endpoints[0].name,
            "POST /api/orders"
        );
    }

    #[tokio::test]
    async fn fixed_clock_makes_users_series_reproducible() {
        use chrono::TimeZone;
        let t = chrono::Utc
            .with_ymd_and_hms(2026, 11, 4, 13, 6, 42)
            .unwrap();
        let q = crate::data::kql::users_sessions_series(TimeRange::H1);
        let a = DemoBackend::at(t).query("x", &q, "PT1H").await.unwrap();
        let b = DemoBackend::at(t).query("x", &q, "PT1H").await.unwrap();
        assert_eq!(a, b);
        assert_eq!(a.len(), 60);
        assert_eq!(a.str(59, "timestamp"), Some("2026-11-04T13:05:00+00:00"));
    }

    #[tokio::test]
    async fn demo_instances_are_made_up() {
        let all = DemoBackend::new().instances().await.unwrap();
        assert_eq!(all.len(), 2);
        assert!(
            all.iter()
                .all(|i| i.subscription_id == "00000000-0000-0000-0000-000000000000")
        );
    }

    #[tokio::test]
    async fn demo_is_deterministic_for_the_same_window() {
        let b = DemoBackend::new();
        let inst = demo_instances().remove(0);
        let now = chrono::Utc::now();
        let a = refresh_dashboard(&b, &inst, TimeRange::H1, now).await;
        let c = refresh_dashboard(&b, &inst, TimeRange::H1, now).await;
        assert_eq!(
            a.series[&crate::data::catalog::SeriesId::RequestsCount],
            c.series[&crate::data::catalog::SeriesId::RequestsCount]
        );
    }
}
