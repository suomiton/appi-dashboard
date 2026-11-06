//! Drill-down screens: what can be opened, how it is loaded, and its data.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use chrono::{DateTime, Utc};

use crate::azure::discovery::AppInstance;
use crate::azure::http::AzError;
use crate::azure::logs::Table;
use crate::azure::metrics::{Aggregation, Interval, MetricSeries, MetricsRequest};
use crate::data::backend::Backend;
use crate::data::dashboard::TopItem;
use crate::data::kql;
use crate::data::time::TimeRange;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstanceMetric {
    Cpu,
    Memory,
}

#[derive(Clone, Debug, PartialEq)]
pub enum DrillTarget {
    FailingEndpoints,
    EndpointFailures {
        name: String,
    },
    RequestDetail {
        operation_id: String,
        request_id: Option<String>,
    },
    ExceptionTypes {
        browser: bool,
    },
    ExceptionOccurrences {
        browser: bool,
        exception_type: String,
    },
    FailingDependencies,
    DependencyCalls {
        dep_type: String,
        target: String,
    },
    SlowEndpoints,
    SlowRequests {
        name: String,
    },
    SlowPages,
    AvailabilityTests,
    AvailabilityFailures {
        test: String,
        location: String,
    },
    InstanceBreakdown {
        metric: InstanceMetric,
    },
}

impl DrillTarget {
    /// The breadcrumb label for this screen.
    pub fn crumb(&self) -> String {
        match self {
            DrillTarget::FailingEndpoints => "failed requests".into(),
            DrillTarget::EndpointFailures { name } => name.clone(),
            DrillTarget::RequestDetail { .. } => "request".into(),
            DrillTarget::ExceptionTypes { browser: false } => "server exceptions".into(),
            DrillTarget::ExceptionTypes { browser: true } => "browser exceptions".into(),
            DrillTarget::ExceptionOccurrences { exception_type, .. } => exception_type.clone(),
            DrillTarget::FailingDependencies => "dependency failures".into(),
            DrillTarget::DependencyCalls { dep_type, target } => format!("{dep_type} {target}"),
            DrillTarget::SlowEndpoints => "slowest endpoints".into(),
            DrillTarget::SlowRequests { name } => name.clone(),
            DrillTarget::SlowPages => "slowest pages".into(),
            DrillTarget::AvailabilityTests => "availability tests".into(),
            DrillTarget::AvailabilityFailures { test, location } => format!("{test} · {location}"),
            DrillTarget::InstanceBreakdown {
                metric: InstanceMetric::Cpu,
            } => "cpu per instance".into(),
            DrillTarget::InstanceBreakdown {
                metric: InstanceMetric::Memory,
            } => "memory per instance".into(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NumFmt {
    Count,
    Ms,
    Pct,
    Bytes,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Cell {
    Text(String),
    Num(f64, NumFmt),
    Time(DateTime<Utc>),
    /// Result codes with counts, most frequent first.
    Codes(Vec<(String, f64)>),
    Trend(Vec<f64>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Normal,
    Warn,
    Bad,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ListRow {
    pub cells: Vec<Cell>,
    pub next: Option<DrillTarget>,
    pub severity: Severity,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ListData {
    pub title: String,
    pub right: String,
    pub columns: Vec<&'static str>,
    pub rows: Vec<ListRow>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DepItem {
    pub dep_type: String,
    pub target: String,
    pub n: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RecentFailure {
    pub at: DateTime<Utc>,
    pub code: String,
    pub duration_ms: f64,
    pub instance: String,
    pub operation_id: String,
    pub request_id: String,
    pub exception_type: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EndpointData {
    pub name: String,
    pub exceptions: Vec<TopItem>,
    pub dependencies: Vec<DepItem>,
    pub recent: Vec<RecentFailure>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RequestInfo {
    pub at: DateTime<Utc>,
    pub name: String,
    pub code: String,
    pub duration_ms: f64,
    pub success: Option<bool>,
    pub instance: String,
    pub operation_id: String,
    pub request_id: String,
    pub session_id: String,
    pub user_id: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExceptionInfo {
    pub exception_type: String,
    pub message: String,
    pub frames: Vec<String>,
}

/// One request or dependency in an operation, as queried.
#[derive(Clone, Debug, PartialEq)]
pub struct TxRow {
    pub at: DateTime<Utc>,
    pub item_type: String,
    pub name: String,
    pub dep_type: String,
    pub target: String,
    pub duration_ms: f64,
    pub success: Option<bool>,
    pub code: String,
    pub id: String,
    pub parent_id: String,
}

/// One line of the end-to-end transaction timeline.
#[derive(Clone, Debug, PartialEq)]
pub struct TxNode {
    pub depth: usize,
    pub label: String,
    pub offset_ms: f64,
    pub duration_ms: f64,
    pub success: Option<bool>,
    pub code: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RequestData {
    pub request: Option<RequestInfo>,
    pub exception: Option<ExceptionInfo>,
    pub transaction: Vec<TxNode>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ScreenData {
    List(ListData),
    Endpoint(EndpointData),
    Request(RequestData),
}

pub fn parse_success(s: &str) -> Option<bool> {
    match s.trim() {
        "True" | "true" | "1" => Some(true),
        "False" | "false" | "0" => Some(false),
        _ => None,
    }
}

/// Orders an operation's rows into a parent → child tree, depth first,
/// children by start time. Rows whose parent is not present are roots;
/// a visited set keeps cycles from looping.
pub fn build_tree(mut rows: Vec<TxRow>) -> Vec<TxNode> {
    rows.sort_by_key(|r| r.at);
    let start = rows.first().map(|r| r.at);
    let ids: std::collections::HashSet<&str> = rows.iter().map(|r| r.id.as_str()).collect();
    let mut children: HashMap<&str, Vec<usize>> = HashMap::new();
    let mut roots = Vec::new();
    for (i, r) in rows.iter().enumerate() {
        if r.parent_id != r.id && ids.contains(r.parent_id.as_str()) {
            children.entry(r.parent_id.as_str()).or_default().push(i);
        } else {
            roots.push(i);
        }
    }
    let mut out = Vec::with_capacity(rows.len());
    let mut visited = vec![false; rows.len()];
    fn walk(
        i: usize,
        depth: usize,
        rows: &[TxRow],
        children: &HashMap<&str, Vec<usize>>,
        visited: &mut [bool],
        out: &mut Vec<(usize, usize)>,
    ) {
        if visited[i] {
            return;
        }
        visited[i] = true;
        out.push((i, depth));
        if let Some(kids) = children.get(rows[i].id.as_str()) {
            for &k in kids {
                walk(k, depth + 1, rows, children, visited, out);
            }
        }
    }
    let mut order = Vec::new();
    for &r in &roots {
        walk(r, 0, &rows, &children, &mut visited, &mut order);
    }
    // Rows only reachable through a cycle become roots themselves.
    for i in 0..rows.len() {
        walk(i, 0, &rows, &children, &mut visited, &mut order);
    }
    for (i, depth) in order {
        let r = &rows[i];
        let label = if r.item_type == "dependency" {
            format!("{} {} {}", r.dep_type, r.target, r.name)
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        } else {
            r.name.clone()
        };
        out.push(TxNode {
            depth,
            label,
            offset_ms: start.map_or(0.0, |s| (r.at - s).num_milliseconds() as f64),
            duration_ms: r.duration_ms,
            success: r.success,
            code: r.code.clone(),
        });
    }
    out
}

/// Readable stack frames from an exception's `details` (an array, or a
/// string holding one): `parsedStack` when present, else `rawStack` lines.
pub fn stack_frames(details: &serde_json::Value, max: usize) -> Vec<String> {
    let parsed;
    let details = match details {
        serde_json::Value::String(s) => match serde_json::from_str::<serde_json::Value>(s) {
            Ok(v) => {
                parsed = v;
                &parsed
            }
            Err(_) => return Vec::new(),
        },
        other => other,
    };
    let Some(items) = details.as_array() else {
        return Vec::new();
    };
    for item in items {
        if let Some(stack) = item.get("parsedStack").and_then(|p| p.as_array())
            && !stack.is_empty()
        {
            return stack
                .iter()
                .take(max)
                .map(|f| {
                    let method = f.get("method").and_then(|m| m.as_str()).unwrap_or("?");
                    let file = f.get("fileName").and_then(|m| m.as_str()).unwrap_or("");
                    let line = f.get("line").and_then(|l| l.as_i64()).unwrap_or(0);
                    match (file.is_empty(), line) {
                        (true, _) => format!("at {method}"),
                        (false, 0) => format!("at {method}  {file}"),
                        (false, l) => format!("at {method}  {file}:{l}"),
                    }
                })
                .collect();
        }
    }
    for item in items {
        if let Some(raw) = item.get("rawStack").and_then(|r| r.as_str()) {
            return raw
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .take(max)
                .map(str::to_string)
                .collect();
        }
    }
    Vec::new()
}

fn pct(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

/// A portal Logs link that opens `kql` on the App Insights resource.
pub fn portal_logs_link(tenant_id: &str, resource_id: &str, kql: &str, timespan: &str) -> String {
    use base64::Engine;
    use std::io::Write;
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gz.write_all(kql.as_bytes())
        .expect("writing gzip to Vec cannot fail");
    let bytes = gz.finish().expect("finishing gzip to Vec cannot fail");
    let q = base64::engine::general_purpose::STANDARD.encode(bytes);
    let tenant = portal_tenant(tenant_id);
    format!(
        "https://portal.azure.com/#{tenant}/blade/Microsoft_Azure_Monitoring_Logs/LogsBlade/resourceId/{}/source/LogsBlade.AnalyticsShareLinkToQuery/q/{}/timespan/{timespan}",
        pct(resource_id),
        pct(&q)
    )
}

fn portal_tenant(tenant_id: &str) -> String {
    if tenant_id.is_empty() {
        String::new()
    } else {
        format!("@{tenant_id}")
    }
}

pub fn portal_resource_link(tenant_id: &str, resource_id: &str) -> String {
    let tenant = portal_tenant(tenant_id);
    format!("https://portal.azure.com/#{tenant}/resource{resource_id}/overview")
}

fn request_data(row: &Table, exceptions: &Table, tx: &Table) -> RequestData {
    let s = |t: &Table, r: usize, c: &str| t.str(r, c).unwrap_or_default().to_string();
    let request = row.time(0, "timestamp").map(|at| RequestInfo {
        at,
        name: s(row, 0, "name"),
        code: s(row, 0, "resultCode"),
        duration_ms: row.f64(0, "duration").unwrap_or(0.0),
        success: parse_success(&s(row, 0, "success")),
        instance: s(row, 0, "cloud_RoleInstance"),
        operation_id: s(row, 0, "operation_Id"),
        request_id: s(row, 0, "id"),
        session_id: s(row, 0, "session_Id"),
        user_id: s(row, 0, "user_Id"),
    });
    let exception = (!exceptions.is_empty()).then(|| {
        let outer = s(exceptions, 0, "outerMessage");
        ExceptionInfo {
            exception_type: s(exceptions, 0, "type"),
            message: if outer.is_empty() {
                s(exceptions, 0, "innermostMessage")
            } else {
                outer
            },
            frames: exceptions
                .json(0, "details")
                .map(|d| stack_frames(d, 8))
                .unwrap_or_default(),
        }
    });
    let rows = (0..tx.len())
        .filter_map(|r| {
            Some(TxRow {
                at: tx.time(r, "timestamp")?,
                item_type: s(tx, r, "itemType"),
                name: s(tx, r, "name"),
                dep_type: s(tx, r, "type"),
                target: s(tx, r, "target"),
                duration_ms: tx.f64(r, "duration").unwrap_or(0.0),
                success: parse_success(&s(tx, r, "success")),
                code: s(tx, r, "resultCode"),
                id: s(tx, r, "id"),
                parent_id: s(tx, r, "operation_ParentId"),
            })
        })
        .collect();
    RequestData {
        request,
        exception,
        transaction: build_tree(rows),
    }
}

/// Builds the endpoint screen from its three query results.
pub fn endpoint_data(name: &str, exceptions: &Table, deps: &Table, recent: &Table) -> EndpointData {
    let s = |t: &Table, r: usize, c: &str| t.str(r, c).unwrap_or_default().to_string();
    EndpointData {
        name: name.to_string(),
        exceptions: (0..exceptions.len())
            .map(|r| TopItem {
                name: s(exceptions, r, "type"),
                detail: s(exceptions, r, "detail"),
                n: exceptions.f64(r, "n").unwrap_or(0.0),
            })
            .collect(),
        dependencies: (0..deps.len())
            .map(|r| DepItem {
                dep_type: s(deps, r, "type"),
                target: s(deps, r, "target"),
                n: deps.f64(r, "n").unwrap_or(0.0),
            })
            .collect(),
        recent: (0..recent.len())
            .filter_map(|r| {
                Some(RecentFailure {
                    at: recent.time(r, "timestamp")?,
                    code: s(recent, r, "resultCode"),
                    duration_ms: recent.f64(r, "duration").unwrap_or(0.0),
                    instance: s(recent, r, "cloud_RoleInstance"),
                    operation_id: s(recent, r, "operation_Id"),
                    request_id: s(recent, r, "id"),
                    exception_type: recent
                        .str(r, "exType")
                        .filter(|t| !t.is_empty())
                        .map(str::to_string),
                })
            })
            .collect(),
    }
}

/// Loads the data for one drill-down screen.
pub async fn load(
    backend: &dyn Backend,
    inst: &AppInstance,
    range: TimeRange,
    target: &DrillTarget,
    now: DateTime<Utc>,
) -> Result<ScreenData, AzError> {
    match target {
        DrillTarget::FailingEndpoints => {
            let t = backend
                .query(&inst.app_id, &kql::failing_endpoints(range), range.iso())
                .await?;
            Ok(ScreenData::List(failing_endpoints_list(&t, range)))
        }
        DrillTarget::EndpointFailures { name } => {
            let (e, d, r) = (
                kql::endpoint_exceptions(name),
                kql::endpoint_dependencies(name),
                kql::endpoint_recent_failures(name),
            );
            let (e, d, r) = tokio::join!(
                backend.query(&inst.app_id, &e, range.iso()),
                backend.query(&inst.app_id, &d, range.iso()),
                backend.query(&inst.app_id, &r, range.iso()),
            );
            Ok(ScreenData::Endpoint(endpoint_data(name, &e?, &d?, &r?)))
        }
        DrillTarget::RequestDetail {
            operation_id,
            request_id,
        } => {
            let (r, e, t) = (
                kql::request_row(operation_id, request_id.as_deref()),
                kql::request_exceptions(operation_id),
                kql::transaction(operation_id),
            );
            let (r, e, t) = tokio::join!(
                backend.query(&inst.app_id, &r, range.iso()),
                backend.query(&inst.app_id, &e, range.iso()),
                backend.query(&inst.app_id, &t, range.iso()),
            );
            Ok(ScreenData::Request(request_data(&r?, &e?, &t?)))
        }
        DrillTarget::ExceptionTypes { browser } => {
            let t = backend
                .query(
                    &inst.app_id,
                    &kql::exception_types(*browser, range),
                    range.iso(),
                )
                .await?;
            Ok(ScreenData::List(exception_types_list(&t, *browser)))
        }
        DrillTarget::ExceptionOccurrences {
            browser,
            exception_type,
        } => {
            let q = kql::exception_occurrences(*browser, exception_type);
            let t = backend.query(&inst.app_id, &q, range.iso()).await?;
            Ok(ScreenData::List(exception_occurrences_list(
                &t,
                *browser,
                exception_type,
            )))
        }
        DrillTarget::FailingDependencies => {
            let t = backend
                .query(&inst.app_id, &kql::failing_dependencies(), range.iso())
                .await?;
            Ok(ScreenData::List(failing_dependencies_list(&t)))
        }
        DrillTarget::DependencyCalls { dep_type, target } => {
            let q = kql::dependency_calls(dep_type, target);
            let t = backend.query(&inst.app_id, &q, range.iso()).await?;
            Ok(ScreenData::List(dependency_calls_list(
                &t, dep_type, target,
            )))
        }
        DrillTarget::SlowEndpoints => {
            let t = backend
                .query(&inst.app_id, &kql::slow_endpoints(), range.iso())
                .await?;
            Ok(ScreenData::List(slow_endpoints_list(&t)))
        }
        DrillTarget::SlowRequests { name } => {
            let t = backend
                .query(&inst.app_id, &kql::slow_requests(name), range.iso())
                .await?;
            Ok(ScreenData::List(slow_requests_list(&t, name)))
        }
        DrillTarget::SlowPages => {
            let t = backend
                .query(&inst.app_id, &kql::slow_pages(), range.iso())
                .await?;
            Ok(ScreenData::List(slow_pages_list(&t)))
        }
        DrillTarget::AvailabilityTests => {
            let t = backend
                .query(&inst.app_id, &kql::availability_tests(), range.iso())
                .await?;
            Ok(ScreenData::List(availability_tests_list(&t)))
        }
        DrillTarget::AvailabilityFailures { test, location } => {
            let q = kql::availability_failures(test, location);
            let t = backend.query(&inst.app_id, &q, range.iso()).await?;
            Ok(ScreenData::List(availability_failures_list(
                &t, test, location,
            )))
        }
        DrillTarget::InstanceBreakdown { metric } => {
            let (name, second) = match metric {
                InstanceMetric::Cpu => (
                    "performanceCounters/processCpuPercentage",
                    Aggregation::Maximum,
                ),
                InstanceMetric::Memory => (
                    "performanceCounters/memoryAvailableBytes",
                    Aggregation::Minimum,
                ),
            };
            let w = crate::data::time::window(range, now);
            let req = |aggregation| MetricsRequest {
                resource_id: inst.resource_id.clone(),
                metric_names: vec![name.to_string()],
                aggregation,
                start: w.start,
                end: w.end,
                interval: Interval::Full,
                split_by: Some("cloud/roleInstance".to_string()),
            };
            let (avg, other) = tokio::join!(
                backend.metrics(req(Aggregation::Average)),
                backend.metrics(req(second)),
            );
            Ok(ScreenData::List(instance_breakdown_list(
                *metric, &avg?, &other?,
            )))
        }
    }
}

fn sort_codes(codes: HashMap<String, f64>) -> Vec<(String, f64)> {
    let mut v: Vec<(String, f64)> = codes.into_iter().collect();
    v.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    v
}

fn severity_for(codes: &[(String, f64)]) -> Severity {
    if codes
        .iter()
        .any(|(c, _)| c.parse::<u16>().is_ok_and(|c| c >= 500))
    {
        Severity::Bad
    } else {
        Severity::Warn
    }
}

/// Groups failed-request rows (name, resultCode, timestamp bin, n) into one
/// row per endpoint, most failures first.
pub fn failing_endpoints_list(t: &Table, range: TimeRange) -> ListData {
    struct Acc {
        fails: f64,
        codes: HashMap<String, f64>,
        bins: BTreeMap<DateTime<Utc>, f64>,
    }
    let mut by_name: HashMap<String, Acc> = HashMap::new();
    let mut all_bins: BTreeSet<DateTime<Utc>> = BTreeSet::new();
    let mut total = 0.0;
    for row in 0..t.len() {
        let name = t.str(row, "name").unwrap_or("(unnamed)").to_string();
        let code = t.str(row, "resultCode").unwrap_or("?").to_string();
        let n = t.f64(row, "n").unwrap_or(0.0);
        total += n;
        let acc = by_name.entry(name).or_insert_with(|| Acc {
            fails: 0.0,
            codes: HashMap::new(),
            bins: BTreeMap::new(),
        });
        acc.fails += n;
        *acc.codes.entry(code).or_insert(0.0) += n;
        if let Some(at) = t.time(row, "timestamp") {
            all_bins.insert(at);
            *acc.bins.entry(at).or_insert(0.0) += n;
        }
    }
    let mut rows: Vec<ListRow> = by_name
        .into_iter()
        .map(|(name, acc)| {
            let codes = sort_codes(acc.codes);
            let trend = all_bins
                .iter()
                .map(|b| acc.bins.get(b).copied().unwrap_or(0.0))
                .collect();
            ListRow {
                severity: severity_for(&codes),
                next: Some(DrillTarget::EndpointFailures { name: name.clone() }),
                cells: vec![
                    Cell::Text(name),
                    Cell::Num(acc.fails, NumFmt::Count),
                    Cell::Codes(codes),
                    Cell::Trend(trend),
                ],
            }
        })
        .collect();
    rows.sort_by(|a, b| {
        sort_key_cmp(&a.cells[1], &b.cells[1]).then_with(|| sort_key_cmp(&a.cells[0], &b.cells[0]))
    });
    ListData {
        title: "failing endpoints".into(),
        right: format!(
            "last {} · {} failed",
            range.label(),
            crate::data::format::fmt_num(total)
        ),
        columns: vec!["endpoint", "fails", "result codes", "trend"],
        rows,
    }
}

/// Orders two cells for sorting: text ascending (case-insensitive), numbers,
/// times, code totals and trend sums descending.
fn sort_key_cmp(a: &Cell, b: &Cell) -> std::cmp::Ordering {
    fn num(c: &Cell) -> f64 {
        match c {
            Cell::Num(v, _) => *v,
            Cell::Time(t) => t.timestamp_millis() as f64,
            Cell::Codes(v) => v.iter().map(|(_, n)| n).sum(),
            Cell::Trend(v) => v.iter().sum(),
            Cell::Text(_) => 0.0,
        }
    }
    match (a, b) {
        (Cell::Text(x), Cell::Text(y)) => x
            .chars()
            .flat_map(char::to_lowercase)
            .cmp(y.chars().flat_map(char::to_lowercase)),
        _ => num(b).total_cmp(&num(a)),
    }
}

/// Sorts rows by one column (see `sort_key_cmp` for direction).
pub fn sort_rows(rows: &mut [ListRow], col: usize) {
    rows.sort_by(|a, b| match (a.cells.get(col), b.cells.get(col)) {
        (Some(x), Some(y)) => sort_key_cmp(x, y),
        _ => std::cmp::Ordering::Equal,
    });
}

/// Rows in a screen, for summaries such as the probe's drill walk.
pub fn row_count(data: &ScreenData) -> usize {
    match data {
        ScreenData::List(l) => l.rows.len(),
        ScreenData::Endpoint(e) => e.exceptions.len() + e.dependencies.len() + e.recent.len(),
        ScreenData::Request(r) => r.transaction.len() + usize::from(r.request.is_some()),
    }
}

fn text(t: &Table, r: usize, c: &str) -> String {
    t.str(r, c).unwrap_or_default().to_string()
}

fn time_cell(t: &Table, r: usize) -> Cell {
    t.time(r, "timestamp")
        .map(Cell::Time)
        .unwrap_or_else(|| Cell::Text(String::new()))
}

fn op_target(t: &Table, r: usize, request_col: Option<&str>) -> Option<DrillTarget> {
    let op = text(t, r, "operation_Id");
    (!op.is_empty()).then(|| DrillTarget::RequestDetail {
        operation_id: op,
        request_id: request_col
            .map(|c| text(t, r, c))
            .filter(|id| !id.is_empty()),
    })
}

pub fn exception_types_list(t: &Table, browser: bool) -> ListData {
    struct Acc {
        n: f64,
        detail: String,
        bins: BTreeMap<DateTime<Utc>, f64>,
    }
    let mut by_type: HashMap<String, Acc> = HashMap::new();
    let mut all_bins = BTreeSet::new();
    for r in 0..t.len() {
        let n = t.f64(r, "n").unwrap_or(0.0);
        let acc = by_type.entry(text(t, r, "type")).or_insert_with(|| Acc {
            n: 0.0,
            detail: String::new(),
            bins: BTreeMap::new(),
        });
        acc.n += n;
        if acc.detail.is_empty() {
            acc.detail = text(t, r, "detail");
        }
        if let Some(at) = t.time(r, "timestamp") {
            all_bins.insert(at);
            *acc.bins.entry(at).or_insert(0.0) += n;
        }
    }
    let mut rows: Vec<ListRow> = by_type
        .into_iter()
        .map(|(ty, acc)| ListRow {
            next: Some(DrillTarget::ExceptionOccurrences {
                browser,
                exception_type: ty.clone(),
            }),
            severity: Severity::Warn,
            cells: vec![
                Cell::Text(ty),
                Cell::Num(acc.n, NumFmt::Count),
                Cell::Text(acc.detail),
                Cell::Trend(
                    all_bins
                        .iter()
                        .map(|b| acc.bins.get(b).copied().unwrap_or(0.0))
                        .collect(),
                ),
            ],
        })
        .collect();
    sort_rows(&mut rows, 1);
    let total: f64 = rows
        .iter()
        .filter_map(|r| match r.cells[1] {
            Cell::Num(v, _) => Some(v),
            _ => None,
        })
        .sum();
    ListData {
        title: if browser {
            "browser exceptions"
        } else {
            "server exceptions"
        }
        .into(),
        right: format!("{} exceptions", crate::data::format::fmt_num(total)),
        columns: vec!["type", "count", "message", "trend"],
        rows,
    }
}

pub fn exception_occurrences_list(t: &Table, browser: bool, exception_type: &str) -> ListData {
    let rows = (0..t.len())
        .map(|r| ListRow {
            next: if browser { None } else { op_target(t, r, None) },
            severity: Severity::Warn,
            cells: vec![
                time_cell(t, r),
                Cell::Text(text(t, r, "operation_Name")),
                Cell::Text(text(t, r, "cloud_RoleInstance")),
                Cell::Text(text(t, r, "outerMessage")),
            ],
        })
        .collect();
    ListData {
        title: exception_type.to_string(),
        right: "latest 50".into(),
        columns: vec!["time", "operation", "instance", "message"],
        rows,
    }
}

pub fn failing_dependencies_list(t: &Table) -> ListData {
    let mut by_dep: HashMap<(String, String), HashMap<String, f64>> = HashMap::new();
    for r in 0..t.len() {
        let codes = by_dep
            .entry((text(t, r, "type"), text(t, r, "target")))
            .or_default();
        *codes.entry(text(t, r, "resultCode")).or_insert(0.0) += t.f64(r, "n").unwrap_or(0.0);
    }
    let mut rows: Vec<ListRow> = by_dep
        .into_iter()
        .map(|((ty, target), codes)| {
            let fails = codes.values().sum();
            let codes = sort_codes(codes);
            ListRow {
                severity: severity_for(&codes),
                next: Some(DrillTarget::DependencyCalls {
                    dep_type: ty.clone(),
                    target: target.clone(),
                }),
                cells: vec![
                    Cell::Text(ty),
                    Cell::Text(target),
                    Cell::Num(fails, NumFmt::Count),
                    Cell::Codes(codes),
                ],
            }
        })
        .collect();
    sort_rows(&mut rows, 2);
    ListData {
        title: "failing dependencies".into(),
        right: format!("{} targets", rows.len()),
        columns: vec!["type", "target", "fails", "result codes"],
        rows,
    }
}

pub fn dependency_calls_list(t: &Table, dep_type: &str, target: &str) -> ListData {
    let rows = (0..t.len())
        .map(|r| ListRow {
            next: op_target(t, r, None),
            severity: Severity::Bad,
            cells: vec![
                time_cell(t, r),
                Cell::Text(text(t, r, "name")),
                Cell::Text(text(t, r, "resultCode")),
                Cell::Num(t.f64(r, "duration").unwrap_or(0.0), NumFmt::Ms),
                Cell::Text(text(t, r, "operation_Name")),
            ],
        })
        .collect();
    ListData {
        title: format!("{dep_type} {target}"),
        right: "latest 50 failed calls".into(),
        columns: vec!["time", "name", "code", "duration", "operation"],
        rows,
    }
}

fn latency_severity(p95: f64) -> Severity {
    if p95 > 3000.0 {
        Severity::Bad
    } else if p95 > 1000.0 {
        Severity::Warn
    } else {
        Severity::Normal
    }
}

fn latency_rows(t: &Table, next: impl Fn(&str) -> Option<DrillTarget>) -> Vec<ListRow> {
    (0..t.len())
        .map(|r| {
            let name = text(t, r, "name");
            let p95 = t.f64(r, "p95").unwrap_or(0.0);
            ListRow {
                next: next(&name),
                severity: latency_severity(p95),
                cells: vec![
                    Cell::Text(name),
                    Cell::Num(t.f64(r, "n").unwrap_or(0.0), NumFmt::Count),
                    Cell::Num(t.f64(r, "avg").unwrap_or(0.0), NumFmt::Ms),
                    Cell::Num(p95, NumFmt::Ms),
                ],
            }
        })
        .collect()
}

pub fn slow_endpoints_list(t: &Table) -> ListData {
    ListData {
        title: "slowest endpoints".into(),
        right: "by p95".into(),
        columns: vec!["endpoint", "count", "avg", "p95"],
        rows: latency_rows(t, |name| {
            Some(DrillTarget::SlowRequests {
                name: name.to_string(),
            })
        }),
    }
}

pub fn slow_pages_list(t: &Table) -> ListData {
    ListData {
        title: "slowest pages".into(),
        right: "by p95".into(),
        columns: vec!["page", "views", "avg", "p95"],
        rows: latency_rows(t, |_| None),
    }
}

pub fn slow_requests_list(t: &Table, name: &str) -> ListData {
    let rows = (0..t.len())
        .map(|r| {
            let d = t.f64(r, "duration").unwrap_or(0.0);
            ListRow {
                next: op_target(t, r, Some("id")),
                severity: latency_severity(d),
                cells: vec![
                    time_cell(t, r),
                    Cell::Text(text(t, r, "resultCode")),
                    Cell::Num(d, NumFmt::Ms),
                    Cell::Text(text(t, r, "cloud_RoleInstance")),
                ],
            }
        })
        .collect();
    ListData {
        title: name.to_string(),
        right: "slowest 50".into(),
        columns: vec!["time", "code", "duration", "instance"],
        rows,
    }
}

pub fn availability_tests_list(t: &Table) -> ListData {
    let rows = (0..t.len())
        .map(|r| {
            let (test, location) = (text(t, r, "name"), text(t, r, "location"));
            let failed = t.f64(r, "failed").unwrap_or(0.0);
            ListRow {
                severity: if failed > 0.0 {
                    Severity::Bad
                } else {
                    Severity::Normal
                },
                next: Some(DrillTarget::AvailabilityFailures {
                    test: test.clone(),
                    location: location.clone(),
                }),
                cells: vec![
                    Cell::Text(test),
                    Cell::Text(location),
                    Cell::Num(t.f64(r, "runs").unwrap_or(0.0), NumFmt::Count),
                    Cell::Num(failed, NumFmt::Count),
                    Cell::Num(t.f64(r, "pct").unwrap_or(0.0), NumFmt::Pct),
                ],
            }
        })
        .collect();
    ListData {
        title: "availability tests".into(),
        right: "per test and location".into(),
        columns: vec!["test", "location", "runs", "failed", "availability"],
        rows,
    }
}

pub fn availability_failures_list(t: &Table, test: &str, location: &str) -> ListData {
    let rows = (0..t.len())
        .map(|r| ListRow {
            next: None,
            severity: Severity::Bad,
            cells: vec![
                time_cell(t, r),
                Cell::Num(t.f64(r, "duration").unwrap_or(0.0), NumFmt::Ms),
                Cell::Text(text(t, r, "message")),
            ],
        })
        .collect();
    ListData {
        title: format!("{test} · {location}"),
        right: "failed runs".into(),
        columns: vec!["time", "duration", "message"],
        rows,
    }
}

pub fn instance_breakdown_list(
    metric: InstanceMetric,
    avg: &[MetricSeries],
    other: &[MetricSeries],
) -> ListData {
    let first = |s: &MetricSeries| s.points.iter().find_map(|(_, v)| *v);
    let fmt = match metric {
        InstanceMetric::Cpu => NumFmt::Pct,
        InstanceMetric::Memory => NumFmt::Bytes,
    };
    let rows = avg
        .iter()
        .filter_map(|s| {
            let name = s.dimension.clone()?;
            let a = first(s)?;
            let o = other
                .iter()
                .find(|x| x.dimension.as_deref() == Some(name.as_str()))
                .and_then(first);
            Some(ListRow {
                next: None,
                severity: Severity::Normal,
                cells: vec![
                    Cell::Text(name),
                    Cell::Num(a, fmt),
                    o.map(|v| Cell::Num(v, fmt))
                        .unwrap_or_else(|| Cell::Text("—".into())),
                ],
            })
        })
        .collect();
    let (title, columns) = match metric {
        InstanceMetric::Cpu => ("cpu per instance", vec!["instance", "avg %", "max %"]),
        InstanceMetric::Memory => (
            "memory per instance",
            vec!["instance", "avg free", "min free"],
        ),
    };
    ListData {
        title: title.into(),
        right: "whole range".into(),
        columns,
        rows,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::data::backend::fake::table;
    use serde_json::json;

    pub(crate) fn endpoints_table() -> Table {
        table(
            &["name", "resultCode", "timestamp", "n"],
            vec![
                json!(["POST /api/orders", "500", "2026-11-04T12:00:00Z", 4]),
                json!(["POST /api/orders", "500", "2026-11-04T12:05:00Z", 5]),
                json!(["POST /api/orders", "502", "2026-11-04T12:05:00Z", 3]),
                json!(["GET /api/reports/export", "504", "2026-11-04T12:00:00Z", 7]),
                json!(["POST /api/auth/token", "401", "2026-11-04T12:10:00Z", 3]),
                json!([
                    "GET /api/sites/{id}/readings",
                    "500",
                    "2026-11-04T12:00:00Z",
                    2
                ]),
            ],
        )
    }

    #[test]
    fn failing_endpoints_list_groups_and_sorts() {
        let l = failing_endpoints_list(&endpoints_table(), TimeRange::H4);
        assert_eq!(l.columns, ["endpoint", "fails", "result codes", "trend"]);
        assert_eq!(l.right, "last 4h · 24 failed");
        assert_eq!(l.rows.len(), 4);
        let first = &l.rows[0];
        assert_eq!(first.cells[0], Cell::Text("POST /api/orders".into()));
        assert_eq!(first.cells[1], Cell::Num(12.0, NumFmt::Count));
        assert_eq!(
            first.cells[2],
            Cell::Codes(vec![("500".into(), 9.0), ("502".into(), 3.0)])
        );
        assert_eq!(first.cells[3], Cell::Trend(vec![4.0, 8.0, 0.0]));
        assert_eq!(first.severity, Severity::Bad);
        assert_eq!(
            first.next,
            Some(DrillTarget::EndpointFailures {
                name: "POST /api/orders".into()
            })
        );
        let auth = l
            .rows
            .iter()
            .find(|r| r.cells[0] == Cell::Text("POST /api/auth/token".into()))
            .unwrap();
        assert_eq!(auth.severity, Severity::Warn);
    }

    pub(crate) fn endpoint_tables() -> (Table, Table, Table) {
        (
            table(
                &["type", "n", "detail"],
                vec![
                    json!(["SqlException", 9, "Execution Timeout Expired"]),
                    json!(["HttpRequestException", 3, "502 (Bad Gateway)"]),
                ],
            ),
            table(
                &["type", "target", "n"],
                vec![json!(["SQL", "demo-sql | appdb", 9])],
            ),
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
                vec![
                    json!([
                        "2026-11-04T12:58:12Z",
                        "500",
                        30020.5,
                        "demo-host-1",
                        "op-1",
                        "req-1",
                        "SqlException"
                    ]),
                    json!([
                        "2026-11-04T12:51:03Z",
                        "502",
                        2100,
                        "demo-host-0",
                        "op-2",
                        "req-2",
                        null
                    ]),
                ],
            ),
        )
    }

    #[test]
    fn endpoint_data_parses_tables() {
        let (e, d, r) = endpoint_tables();
        let data = endpoint_data("POST /api/orders", &e, &d, &r);
        assert_eq!(data.name, "POST /api/orders");
        assert_eq!(data.exceptions[0].name, "SqlException");
        assert_eq!(data.exceptions[0].n, 9.0);
        assert_eq!(data.exceptions[1].detail, "502 (Bad Gateway)");
        assert_eq!(
            data.dependencies[0],
            DepItem {
                dep_type: "SQL".into(),
                target: "demo-sql | appdb".into(),
                n: 9.0
            }
        );
        assert_eq!(data.recent.len(), 2);
        assert_eq!(data.recent[0].duration_ms, 30020.5);
        assert_eq!(data.recent[0].operation_id, "op-1");
        assert_eq!(
            data.recent[0].exception_type.as_deref(),
            Some("SqlException")
        );
        assert_eq!(data.recent[1].exception_type, None);
    }

    #[tokio::test]
    async fn load_endpoint_runs_three_queries() {
        let (e, d, r) = endpoint_tables();
        let name = "POST /api/orders";
        let b = crate::data::backend::fake::FakeBackend::new()
            .with_query(kql::endpoint_exceptions(name), Ok(e))
            .with_query(kql::endpoint_dependencies(name), Ok(d))
            .with_query(kql::endpoint_recent_failures(name), Ok(r));
        let inst = crate::data::demo::demo_instances().remove(0);
        let target = DrillTarget::EndpointFailures { name: name.into() };
        let data = load(&b, &inst, TimeRange::H4, &target, chrono::Utc::now())
            .await
            .unwrap();
        assert!(matches!(data, ScreenData::Endpoint(d) if d.recent.len() == 2));
        assert_eq!(b.calls().len(), 3);
    }

    fn tx(id: &str, parent: &str, at_ms: i64, item: &str, name: &str) -> TxRow {
        use chrono::TimeZone;
        TxRow {
            at: Utc.with_ymd_and_hms(2026, 11, 4, 12, 0, 0).unwrap()
                + chrono::Duration::milliseconds(at_ms),
            item_type: item.into(),
            name: name.into(),
            dep_type: if item == "dependency" {
                "SQL".into()
            } else {
                String::new()
            },
            target: if item == "dependency" {
                "appdb".into()
            } else {
                String::new()
            },
            duration_ms: 10.0,
            success: Some(true),
            code: "200".into(),
            id: id.into(),
            parent_id: parent.into(),
        }
    }

    #[test]
    fn build_tree_orders_depth_first() {
        let nodes = build_tree(vec![
            tx("d2", "r1", 10, "dependency", "d2"),
            tx("r1", "op", 0, "request", "R"),
            tx("d3", "d1", 6, "dependency", "d3"),
            tx("d1", "r1", 5, "dependency", "d1"),
        ]);
        let labels: Vec<&str> = nodes.iter().map(|n| n.label.as_str()).collect();
        assert_eq!(
            labels,
            ["R", "SQL appdb d1", "SQL appdb d3", "SQL appdb d2"]
        );
        assert_eq!(
            nodes.iter().map(|n| n.depth).collect::<Vec<_>>(),
            [0, 1, 2, 1]
        );
        assert_eq!(
            nodes.iter().map(|n| n.offset_ms).collect::<Vec<_>>(),
            [0.0, 5.0, 6.0, 10.0]
        );
    }

    #[test]
    fn build_tree_handles_orphans_and_cycles() {
        let nodes = build_tree(vec![
            tx("a", "b", 0, "request", "a"),
            tx("b", "a", 1, "request", "b"),
            tx("c", "missing", 2, "request", "c"),
        ]);
        assert_eq!(nodes.len(), 3);
        let mut labels: Vec<&str> = nodes.iter().map(|n| n.label.as_str()).collect();
        labels.sort();
        assert_eq!(labels, ["a", "b", "c"]);
    }

    #[test]
    fn stack_frames_from_parsed_stack() {
        let details = json!([{"parsedStack": [
            {"level": 0, "method": "Demo.OrderService.SaveAsync", "fileName": "OrderService.cs", "line": 142},
            {"level": 1, "method": "Demo.OrdersController.Create", "line": 0},
        ]}]);
        assert_eq!(
            stack_frames(&details, 8),
            [
                "at Demo.OrderService.SaveAsync  OrderService.cs:142",
                "at Demo.OrdersController.Create",
            ]
        );
    }

    #[test]
    fn stack_frames_from_json_string_and_raw_stack() {
        let as_string = serde_json::Value::String(
            r#"[{"parsedStack":[{"method":"M","fileName":"F.cs","line":3}]}]"#.into(),
        );
        assert_eq!(stack_frames(&as_string, 8), ["at M  F.cs:3"]);
        let raw =
            json!([{"rawStack": "Error: boom\n    at f (app.js:1:2)\n\n    at g (app.js:3:4)\n"}]);
        assert_eq!(stack_frames(&raw, 2), ["Error: boom", "at f (app.js:1:2)"]);
        assert!(stack_frames(&json!(null), 8).is_empty());
    }

    #[test]
    fn stack_frames_respects_max() {
        let frames: Vec<_> = (0..20)
            .map(|i| json!({"method": format!("m{i}")}))
            .collect();
        assert_eq!(stack_frames(&json!([{"parsedStack": frames}]), 5).len(), 5);
    }

    #[test]
    fn parse_success_variants() {
        for t in ["True", "true", "1"] {
            assert_eq!(parse_success(t), Some(true));
        }
        for f in ["False", "false", "0"] {
            assert_eq!(parse_success(f), Some(false));
        }
        assert_eq!(parse_success(""), None);
    }

    #[test]
    fn portal_logs_link_roundtrip() {
        use base64::Engine;
        use std::io::Read;
        let kql = "union requests, dependencies | where operation_Id == \"op-1\"";
        let rid = "/subscriptions/00000000-0000-0000-0000-000000000000/resourceGroups/demo-rg/providers/microsoft.insights/components/demo-app-prod";
        let link = portal_logs_link("tenant-1", rid, kql, "PT4H");
        assert!(link.starts_with("https://portal.azure.com/#@tenant-1/blade/Microsoft_Azure_Monitoring_Logs/LogsBlade/resourceId/%2Fsubscriptions%2F"), "{link}");
        assert!(link.ends_with("/timespan/PT4H"));
        let q = link.split("/q/").nth(1).unwrap().split('/').next().unwrap();
        let decoded = url::form_urlencoded::parse(format!("x={q}").as_bytes())
            .next()
            .unwrap()
            .1
            .to_string();
        let gz = base64::engine::general_purpose::STANDARD
            .decode(decoded)
            .unwrap();
        let mut out = String::new();
        flate2::read::GzDecoder::new(&gz[..])
            .read_to_string(&mut out)
            .unwrap();
        assert_eq!(out, kql);
        assert_eq!(
            portal_resource_link("tenant-1", rid),
            format!("https://portal.azure.com/#@tenant-1/resource{rid}/overview")
        );
    }

    #[test]
    fn portal_links_allow_an_unspecified_tenant() {
        let resource =
            "/subscriptions/x/resourceGroups/rg/providers/Microsoft.Insights/components/app";
        let link = portal_logs_link("", resource, "requests | take 1", "PT4H");
        assert!(
            link.starts_with("https://portal.azure.com/#/blade/"),
            "{link}"
        );
        assert!(!link.contains("#@/"));
        assert_eq!(
            portal_resource_link("", resource),
            format!("https://portal.azure.com/#/resource{resource}/overview")
        );
    }

    #[tokio::test]
    async fn load_request_detail_builds_all_parts() {
        let b = crate::data::demo::DemoBackend::new();
        let inst = crate::data::demo::demo_instances().remove(0);
        let target = DrillTarget::RequestDetail {
            operation_id: "op-7f3a9c2e".into(),
            request_id: Some("req-0001".into()),
        };
        let ScreenData::Request(r) = load(&b, &inst, TimeRange::H4, &target, chrono::Utc::now())
            .await
            .unwrap()
        else {
            panic!("expected request data")
        };
        let req = r.request.unwrap();
        assert_eq!(req.code, "500");
        assert_eq!(req.success, Some(false));
        assert_eq!(r.exception.unwrap().exception_type, "SqlException");
        assert_eq!(r.transaction.len(), 3);
        assert_eq!(r.transaction[0].depth, 0);
    }

    fn t(at: &str) -> serde_json::Value {
        json!(format!("2026-11-04T{at}Z"))
    }

    #[test]
    fn exception_types_list_test() {
        let tb = table(
            &["type", "detail", "timestamp", "n"],
            vec![
                json!(["SqlException", "Timeout", t("12:00:00"), 4]),
                json!(["SqlException", "", t("12:05:00"), 5]),
                json!(["TypeError", "x is undefined", t("12:05:00"), 2]),
            ],
        );
        let l = exception_types_list(&tb, false);
        assert_eq!(l.columns, ["type", "count", "message", "trend"]);
        assert_eq!(l.rows[0].cells[1], Cell::Num(9.0, NumFmt::Count));
        assert_eq!(l.rows[0].cells[2], Cell::Text("Timeout".into()));
        assert_eq!(l.rows[0].cells[3], Cell::Trend(vec![4.0, 5.0]));
        assert_eq!(
            l.rows[0].next,
            Some(DrillTarget::ExceptionOccurrences {
                browser: false,
                exception_type: "SqlException".into()
            })
        );
        assert_eq!(exception_types_list(&tb, true).title, "browser exceptions");
    }

    #[test]
    fn exception_occurrences_list_test() {
        let tb = table(
            &[
                "timestamp",
                "operation_Name",
                "cloud_RoleInstance",
                "outerMessage",
                "operation_Id",
            ],
            vec![
                json!([t("12:00:00"), "POST /x", "host-0", "boom", "op-1"]),
                json!([t("12:01:00"), "", "", "boom", ""]),
            ],
        );
        let l = exception_occurrences_list(&tb, false, "SqlException");
        assert_eq!(l.columns, ["time", "operation", "instance", "message"]);
        assert_eq!(
            l.rows[0].next,
            Some(DrillTarget::RequestDetail {
                operation_id: "op-1".into(),
                request_id: None
            })
        );
        assert_eq!(l.rows[1].next, None, "no operation id: nothing to open");
        assert_eq!(
            exception_occurrences_list(&tb, true, "TypeError").rows[0].next,
            None
        );
    }

    #[test]
    fn failing_dependencies_list_test() {
        let tb = table(
            &["type", "target", "resultCode", "n"],
            vec![
                json!(["SQL", "appdb", "-2", 9]),
                json!(["HTTP", "func", "502", 2]),
                json!(["HTTP", "func", "503", 1]),
            ],
        );
        let l = failing_dependencies_list(&tb);
        assert_eq!(l.columns, ["type", "target", "fails", "result codes"]);
        assert_eq!(l.rows[0].cells[2], Cell::Num(9.0, NumFmt::Count));
        assert_eq!(l.rows[0].severity, Severity::Warn);
        assert_eq!(l.rows[1].severity, Severity::Bad);
        assert_eq!(
            l.rows[1].next,
            Some(DrillTarget::DependencyCalls {
                dep_type: "HTTP".into(),
                target: "func".into()
            })
        );
    }

    #[test]
    fn dependency_calls_list_test() {
        let tb = table(
            &[
                "timestamp",
                "name",
                "resultCode",
                "duration",
                "operation_Id",
                "operation_Name",
            ],
            vec![json!([
                t("12:00:00"),
                "INSERT",
                "-2",
                29950.0,
                "op-1",
                "POST /x"
            ])],
        );
        let l = dependency_calls_list(&tb, "SQL", "appdb");
        assert_eq!(l.columns, ["time", "name", "code", "duration", "operation"]);
        assert_eq!(l.rows[0].cells[3], Cell::Num(29950.0, NumFmt::Ms));
        assert_eq!(
            l.rows[0].next,
            Some(DrillTarget::RequestDetail {
                operation_id: "op-1".into(),
                request_id: None
            })
        );
    }

    #[test]
    fn slow_endpoints_and_pages_lists() {
        let tb = table(
            &["name", "n", "avg", "p95"],
            vec![
                json!(["GET /slow", 10, 2300.0, 8100.0]),
                json!(["GET /fast", 99, 20.0, 40.0]),
            ],
        );
        let l = slow_endpoints_list(&tb);
        assert_eq!(l.columns, ["endpoint", "count", "avg", "p95"]);
        assert_eq!(l.rows[0].cells[3], Cell::Num(8100.0, NumFmt::Ms));
        assert_eq!(l.rows[0].severity, Severity::Bad);
        assert_eq!(l.rows[1].severity, Severity::Normal);
        assert_eq!(
            l.rows[0].next,
            Some(DrillTarget::SlowRequests {
                name: "GET /slow".into()
            })
        );
        let p = slow_pages_list(&tb);
        assert_eq!(p.columns, ["page", "views", "avg", "p95"]);
        assert_eq!(p.rows[0].next, None);
    }

    #[test]
    fn slow_requests_list_test() {
        let tb = table(
            &[
                "timestamp",
                "resultCode",
                "duration",
                "cloud_RoleInstance",
                "operation_Id",
                "id",
            ],
            vec![json!([
                t("12:00:00"),
                "200",
                8100.0,
                "host-0",
                "op-1",
                "req-1"
            ])],
        );
        let l = slow_requests_list(&tb, "GET /slow");
        assert_eq!(l.columns, ["time", "code", "duration", "instance"]);
        assert_eq!(
            l.rows[0].next,
            Some(DrillTarget::RequestDetail {
                operation_id: "op-1".into(),
                request_id: Some("req-1".into())
            })
        );
    }

    #[test]
    fn availability_lists() {
        let tb = table(
            &["name", "location", "runs", "failed", "pct"],
            vec![
                json!(["home", "West Europe", 48, 1, 97.9]),
                json!(["home", "North Europe", 48, 0, 100.0]),
            ],
        );
        let l = availability_tests_list(&tb);
        assert_eq!(
            l.columns,
            ["test", "location", "runs", "failed", "availability"]
        );
        assert_eq!(l.rows[0].cells[4], Cell::Num(97.9, NumFmt::Pct));
        assert_eq!(l.rows[0].severity, Severity::Bad);
        assert_eq!(l.rows[1].severity, Severity::Normal);
        assert_eq!(
            l.rows[0].next,
            Some(DrillTarget::AvailabilityFailures {
                test: "home".into(),
                location: "West Europe".into()
            })
        );
        let f = table(
            &["timestamp", "duration", "message"],
            vec![json!([t("12:00:00"), 5000.0, "timeout"])],
        );
        let fl = availability_failures_list(&f, "home", "West Europe");
        assert_eq!(fl.columns, ["time", "duration", "message"]);
        assert_eq!(fl.rows[0].next, None);
    }

    #[test]
    fn instance_breakdown_list_test() {
        use crate::azure::metrics::MetricSeries;
        let s = |dim: &str, v: f64| MetricSeries {
            name: "m".into(),
            dimension: Some(dim.into()),
            points: vec![(Utc::now(), Some(v))],
        };
        let l = instance_breakdown_list(
            InstanceMetric::Cpu,
            &[s("h0", 1.5), s("h1", 0.5)],
            &[s("h0", 9.0), s("h1", 2.0)],
        );
        assert_eq!(l.columns, ["instance", "avg %", "max %"]);
        assert_eq!(
            l.rows[0].cells,
            vec![
                Cell::Text("h0".into()),
                Cell::Num(1.5, NumFmt::Pct),
                Cell::Num(9.0, NumFmt::Pct)
            ]
        );
        let m = instance_breakdown_list(InstanceMetric::Memory, &[s("h0", 1e9)], &[s("h0", 5e8)]);
        assert_eq!(m.columns, ["instance", "avg free", "min free"]);
        assert_eq!(m.rows[0].cells[2], Cell::Num(5e8, NumFmt::Bytes));
    }

    #[tokio::test]
    async fn demo_backend_answers_every_target() {
        let b = crate::data::demo::DemoBackend::new();
        let inst = crate::data::demo::demo_instances().remove(0);
        let targets = [
            DrillTarget::FailingEndpoints,
            DrillTarget::ExceptionTypes { browser: false },
            DrillTarget::ExceptionTypes { browser: true },
            DrillTarget::ExceptionOccurrences {
                browser: false,
                exception_type: "SqlException".into(),
            },
            DrillTarget::ExceptionOccurrences {
                browser: true,
                exception_type: "TypeError".into(),
            },
            DrillTarget::FailingDependencies,
            DrillTarget::DependencyCalls {
                dep_type: "SQL".into(),
                target: "demo-sql.database.windows.net | appdb".into(),
            },
            DrillTarget::SlowEndpoints,
            DrillTarget::SlowRequests {
                name: "POST /api/orders".into(),
            },
            DrillTarget::SlowPages,
            DrillTarget::AvailabilityTests,
            DrillTarget::AvailabilityFailures {
                test: "home page".into(),
                location: "West Europe".into(),
            },
            DrillTarget::InstanceBreakdown {
                metric: InstanceMetric::Cpu,
            },
            DrillTarget::InstanceBreakdown {
                metric: InstanceMetric::Memory,
            },
        ];
        for target in targets {
            let data = load(&b, &inst, TimeRange::H4, &target, Utc::now())
                .await
                .unwrap();
            assert!(row_count(&data) > 0, "{target:?} has rows");
        }
    }

    #[test]
    fn empty_table_gives_empty_list() {
        let l = failing_endpoints_list(&Table::default(), TimeRange::H1);
        assert!(l.rows.is_empty());
        assert_eq!(l.right, "last 1h · 0 failed");
    }

    #[test]
    fn crumbs() {
        assert_eq!(DrillTarget::FailingEndpoints.crumb(), "failed requests");
        assert_eq!(
            DrillTarget::EndpointFailures {
                name: "POST /x".into()
            }
            .crumb(),
            "POST /x"
        );
        assert_eq!(
            DrillTarget::InstanceBreakdown {
                metric: InstanceMetric::Cpu
            }
            .crumb(),
            "cpu per instance"
        );
    }

    #[test]
    fn sorting_rows() {
        let mut l = failing_endpoints_list(&endpoints_table(), TimeRange::H4);
        sort_rows(&mut l.rows, 0);
        assert_eq!(
            l.rows[0].cells[0],
            Cell::Text("GET /api/reports/export".into())
        );
        sort_rows(&mut l.rows, 1);
        assert_eq!(l.rows[0].cells[1], Cell::Num(12.0, NumFmt::Count));
    }

    #[tokio::test]
    async fn load_failing_endpoints_uses_range_timespan() {
        let b = crate::data::backend::fake::FakeBackend::new().with_query(
            crate::data::kql::failing_endpoints(TimeRange::H4),
            Ok(endpoints_table()),
        );
        let inst = crate::data::demo::demo_instances().remove(0);
        let data = load(
            &b,
            &inst,
            TimeRange::H4,
            &DrillTarget::FailingEndpoints,
            chrono::Utc::now(),
        )
        .await
        .unwrap();
        assert!(matches!(data, ScreenData::List(l) if l.rows.len() == 4));
        assert!(b.calls().iter().any(|c| c.starts_with("query:PT4H:")));
    }
}
