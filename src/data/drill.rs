//! Drill-down screens: what can be opened, how it is loaded, and its data.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use chrono::{DateTime, Utc};

use crate::azure::discovery::AppInstance;
use crate::azure::http::AzError;
use crate::azure::logs::Table;
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
pub enum ScreenData {
    List(ListData),
    Endpoint(EndpointData),
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
    _now: DateTime<Utc>,
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
        _ => Err(AzError::Decode("not implemented".into())),
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
        (Cell::Text(x), Cell::Text(y)) => x.to_lowercase().cmp(&y.to_lowercase()),
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
