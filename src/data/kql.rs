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
    "let eps = requests | where timestamp > ago(15m) and tostring(success) == \"False\" | summarize n = sum(itemCount) by name | top 5 by n | extend section = \"endpoint\", detail = \"\";
let exs = exceptions | where timestamp > ago(15m) and client_Type != \"Browser\" | summarize n = sum(itemCount), detail = take_any(outerMessage) by name = type | top 5 by n | extend section = \"exception\";
union eps, exs | project section, name, detail, n"
        .to_string()
}

pub fn failing_endpoints(range: TimeRange) -> String {
    let g = range.grain_minutes();
    format!(
        "requests | where tostring(success) == \"False\"
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
        "dependencies | where operation_Name == {x} and tostring(success) == \"False\" | summarize n = sum(itemCount) by type, target | top 10 by n"
    )
}

pub fn endpoint_recent_failures(name: &str) -> String {
    let x = kql_string(name);
    format!(
        "requests | where name == {x} and tostring(success) == \"False\"
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
    fn dashboard_queries_snapshot() {
        insta::assert_snapshot!("summary_scalars", summary_scalars());
        insta::assert_snapshot!("failure_tops", failure_tops());
    }
}
