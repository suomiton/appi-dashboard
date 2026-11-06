//! KQL builders. User-controlled strings always go through `kql_string`.

use crate::data::time::TimeRange;

/// Microsoft's definition of the log-based users and sessions metrics.
pub const ALL_TABLES: &str = "union traces, requests, pageViews, dependencies, customEvents, availabilityResults, exceptions, customMetrics, browserTimings";

/// A double-quoted KQL string literal.
pub fn kql_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

pub fn summary_scalars() -> String {
    format!(
        "let all = {ALL_TABLES};
print users_5m = toscalar(all | where timestamp > ago(5m) and notempty(user_Id) | summarize dcount(user_Id)),
      users_range = toscalar(all | where notempty(user_Id) | summarize dcount(user_Id)),
      sessions_range = toscalar(all | where notempty(session_Id) | summarize dcount(session_Id)),
      servers_10m = toscalar(union requests, performanceCounters | where timestamp > ago(10m) and notempty(cloud_RoleInstance) | summarize dcount(cloud_RoleInstance)),
      response_p95_ms = toscalar(requests | summarize percentile(duration, 95))"
    )
}

pub fn users_sessions_series(range: TimeRange) -> String {
    let g = range.grain_minutes();
    format!(
        "{ALL_TABLES}
| summarize users = dcountif(user_Id, notempty(user_Id)), sessions = dcountif(session_Id, notempty(session_Id)) by bin(timestamp, {g}m)
| order by timestamp asc"
    )
}

pub fn failure_tops() -> String {
    format!("let eps = requests | where timestamp > ago(15m) and {FAILED} | summarize n = sum(itemCount) by name | top 5 by n | extend section = \"endpoint\", detail = \"\";
let exs = exceptions | where timestamp > ago(15m) and client_Type != \"Browser\" | summarize n = sum(itemCount), detail = take_any(outerMessage) by name = type | top 5 by n | extend section = \"exception\";
union eps, exs | project section, name, detail, n")
}

pub fn failing_endpoints(range: TimeRange) -> String {
    let g = range.grain_minutes();
    format!(
        "requests | where {FAILED}
| summarize n = sum(itemCount) by name, resultCode, bin(timestamp, {g}m)"
    )
}

pub fn endpoint_exceptions(name: &str) -> String {
    let x = kql_string(name);
    format!(
        "exceptions | where operation_Name == {x} | summarize n = sum(itemCount), detail = take_any(outerMessage) by type | top 10 by n"
    )
}

pub fn endpoint_dependencies(name: &str) -> String {
    let x = kql_string(name);
    format!(
        "dependencies | where operation_Name == {x} and {FAILED} | summarize n = sum(itemCount) by type, target | top 10 by n"
    )
}

pub fn endpoint_recent_failures(name: &str) -> String {
    let x = kql_string(name);
    format!(
        "requests | where name == {x} and {FAILED}
| top 50 by timestamp desc
| project timestamp, resultCode, duration, cloud_RoleInstance, operation_Id, id
| join kind=leftouter (exceptions | where operation_Name == {x} | summarize exType = take_any(type) by operation_Id) on operation_Id
| project timestamp, resultCode, duration, cloud_RoleInstance, operation_Id, id, exType
| order by timestamp desc"
    )
}

pub fn request_row(operation_id: &str, request_id: Option<&str>) -> String {
    let o = kql_string(operation_id);
    let id_filter = request_id
        .map(|r| format!(" and id == {}", kql_string(r)))
        .unwrap_or_default();
    format!(
        "requests | where operation_Id == {o}{id_filter} | top 1 by timestamp desc
| project timestamp, name, resultCode, duration, success = tostring(success), cloud_RoleInstance, operation_Id, id, session_Id, user_Id"
    )
}

pub fn request_exceptions(operation_id: &str) -> String {
    let o = kql_string(operation_id);
    format!(
        "exceptions | where operation_Id == {o} | top 5 by timestamp asc | project timestamp, type, outerMessage, innermostMessage, details"
    )
}

pub fn transaction(operation_id: &str) -> String {
    let o = kql_string(operation_id);
    format!(
        "union requests, dependencies | where operation_Id == {o}
| project timestamp, itemType, name, type, target, duration, success = tostring(success), resultCode, id, operation_ParentId
| order by timestamp asc | take 200"
    )
}

fn browser_filter(browser: bool) -> &'static str {
    if browser {
        "client_Type == \"Browser\""
    } else {
        "client_Type != \"Browser\""
    }
}

const FAILED: &str = "tostring(success) in (\"0\", \"False\", \"false\")";

pub fn exception_types(browser: bool, range: TimeRange) -> String {
    let g = range.grain_minutes();
    let f = browser_filter(browser);
    format!(
        "exceptions | where {f}
| summarize n = sum(itemCount), detail = take_any(outerMessage) by type, bin(timestamp, {g}m)"
    )
}

pub fn exception_occurrences(browser: bool, exception_type: &str) -> String {
    let f = browser_filter(browser);
    let t = kql_string(exception_type);
    format!(
        "exceptions | where type == {t} and {f}
| top 50 by timestamp desc
| project timestamp, operation_Name, cloud_RoleInstance, outerMessage, operation_Id"
    )
}

pub fn failing_dependencies() -> String {
    format!(
        "dependencies | where {FAILED}
| summarize n = sum(itemCount) by type, target, resultCode"
    )
}

pub fn dependency_calls(dep_type: &str, target: &str) -> String {
    let (t, g) = (kql_string(dep_type), kql_string(target));
    format!(
        "dependencies | where type == {t} and target == {g} and {FAILED}
| top 50 by timestamp desc
| project timestamp, name, resultCode, duration, operation_Id, operation_Name"
    )
}

pub fn slow_endpoints() -> String {
    "requests
| summarize n = sum(itemCount), avg = sum(duration * itemCount) / sum(itemCount), p95 = percentile(duration, 95) by name
| top 50 by p95 desc"
        .to_string()
}

pub fn slow_requests(name: &str) -> String {
    let x = kql_string(name);
    format!(
        "requests | where name == {x}
| top 50 by duration desc
| project timestamp, resultCode, duration, cloud_RoleInstance, operation_Id, id"
    )
}

pub fn slow_pages() -> String {
    "pageViews
| summarize n = sum(itemCount), avg = sum(duration * itemCount) / sum(itemCount), p95 = percentile(duration, 95) by name
| top 50 by p95 desc"
        .to_string()
}

pub fn availability_tests() -> String {
    format!(
        "availabilityResults
| summarize runs = sum(itemCount), failed = sumif(itemCount, {FAILED}) by name, location
| extend pct = 100.0 * (runs - failed) / runs
| order by pct asc"
    )
}

pub fn availability_failures(test: &str, location: &str) -> String {
    let (n, l) = (kql_string(test), kql_string(location));
    format!(
        "availabilityResults | where name == {n} and location == {l} and {FAILED}
| top 50 by timestamp desc
| project timestamp, duration, message"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::time::TimeRange;

    #[test]
    fn string_literals_are_escaped() {
        assert_eq!(kql_string(r#"GET /a"b\c"#), r#""GET /a\"b\\c""#);
        assert_eq!(kql_string("a\nb"), r#""a\nb""#);
    }

    #[test]
    fn injection_is_neutralized() {
        let q = endpoint_recent_failures(r#"x" | take 1 //"#);
        assert!(q.contains(r#"name == "x\" | take 1 //""#));
    }

    #[test]
    fn series_bin_follows_range_grain() {
        assert!(users_sessions_series(TimeRange::H4).contains("bin(timestamp, 5m)"));
        assert!(users_sessions_series(TimeRange::M30).contains("bin(timestamp, 1m)"));
        assert!(failing_endpoints(TimeRange::H24).contains("bin(timestamp, 15m)"));
    }

    #[test]
    fn request_row_optional_id() {
        assert!(!request_row("o", None).contains("id =="));
        assert!(request_row("o", Some("r")).contains(r#"and id == "r""#));
    }

    #[test]
    fn builders_use_their_inputs() {
        assert!(endpoint_exceptions("POST /x").contains(r#"operation_Name == "POST /x""#));
        assert!(endpoint_dependencies("POST /x").contains(r#"operation_Name == "POST /x""#));
        assert!(request_exceptions("op1").contains(r#"operation_Id == "op1""#));
        assert!(transaction("op1").contains(r#"operation_Id == "op1""#));
        assert!(ALL_TABLES.starts_with("union traces, requests, pageViews"));
    }

    /// `kind` is a reserved word in KQL (`join kind=...`); using it as a
    /// column name makes the App Insights API reject the whole query.
    #[test]
    fn failure_tops_avoids_reserved_word_kind() {
        assert!(!failure_tops().contains("kind"));
        assert!(failure_tops().contains("section"));
    }

    #[test]
    fn drill_builders_escape_inputs() {
        let evil = r#"x" | take 1"#;
        let esc = r#""x\" | take 1""#;
        assert!(exception_occurrences(false, evil).contains(esc));
        assert!(dependency_calls(evil, "t").contains(esc));
        assert!(dependency_calls("t", evil).contains(esc));
        assert!(slow_requests(evil).contains(esc));
        assert!(availability_failures(evil, "l").contains(esc));
        assert!(availability_failures("n", evil).contains(esc));
    }

    #[test]
    fn all_failure_queries_accept_false_spellings() {
        for q in [
            failure_tops(),
            failing_endpoints(TimeRange::H4),
            endpoint_dependencies("x"),
            endpoint_recent_failures("x"),
            failing_dependencies(),
            dependency_calls("t", "g"),
            availability_tests(),
            availability_failures("n", "l"),
        ] {
            assert!(q.contains(FAILED), "{q}");
            assert!(!q.contains("== \"False\""));
        }
    }

    #[test]
    fn browser_filter_and_bins() {
        assert!(exception_types(true, TimeRange::H4).contains(r#"client_Type == "Browser""#));
        assert!(exception_types(false, TimeRange::H4).contains(r#"client_Type != "Browser""#));
        assert!(exception_types(false, TimeRange::H24).contains("bin(timestamp, 15m)"));
        assert!(exception_occurrences(true, "T").contains(r#"client_Type == "Browser""#));
        for q in [
            failing_dependencies(),
            slow_endpoints(),
            slow_pages(),
            availability_tests(),
        ] {
            assert!(!q.contains("kind"), "reserved word");
        }
    }

    #[test]
    fn dashboard_queries_snapshot() {
        insta::assert_snapshot!("summary_scalars", summary_scalars());
        insta::assert_snapshot!("failure_tops", failure_tops());
    }
}
