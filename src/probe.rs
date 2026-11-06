//! `appi-dashboard probe`: checks Azure access and prints what a refresh returns.
//!
//! Output never contains tokens, tenant IDs or subscription IDs.

use std::io::Write;

use crate::azure::auth::{Audience, TokenSource};
use crate::azure::discovery::AppInstance;
use crate::data::backend::Backend;
use crate::data::catalog::{AVERAGE_METRICS, COUNT_METRICS};
use crate::data::dashboard::{Source, refresh_dashboard};
use crate::data::drill::{DrillTarget, InstanceMetric, ScreenData, load, row_count};
use crate::data::time::TimeRange;

fn num(v: f64) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn source_name(s: Source) -> &'static str {
    match s {
        Source::CountMetrics => "count metrics",
        Source::AverageMetrics => "average metrics",
        Source::CountTotals => "count totals",
        Source::AverageTotals => "average totals",
        Source::AlertWindow => "alert window",
        Source::Scalars => "summary",
        Source::UsersSessions => "users/sessions series",
        Source::Tops => "failure tops",
    }
}

fn list(out: &mut dyn Write, instances: &[&AppInstance]) -> std::io::Result<()> {
    for i in instances {
        writeln!(
            out,
            "     {}  ({} · {})",
            i.name, i.resource_group, i.subscription_name
        )?;
    }
    Ok(())
}

/// Runs the checks and writes one line per check. Returns true when no
/// line is a `[FAIL]`.
pub async fn run_probe(
    tokens: &dyn TokenSource,
    backend: &dyn Backend,
    app: Option<&str>,
    range: TimeRange,
    out: &mut dyn Write,
) -> std::io::Result<bool> {
    let mut ok = true;
    for (aud, label) in [
        (Audience::Arm, "arm"),
        (Audience::AppInsights, "app insights"),
    ] {
        match tokens.token(aud).await {
            Ok(_) => writeln!(out, "[ok] token {label}")?,
            Err(e) => {
                ok = false;
                writeln!(out, "[FAIL] token {label}: {e}")?;
            }
        }
    }

    let all = match backend.instances().await {
        Ok(all) => {
            writeln!(out, "[ok] discovery: {} instances", all.len())?;
            all
        }
        Err(e) => {
            writeln!(out, "[FAIL] discovery: {e}")?;
            return Ok(false);
        }
    };

    let Some(app) = app else {
        list(out, &all.iter().collect::<Vec<_>>())?;
        return Ok(ok);
    };
    let matches: Vec<&AppInstance> = all
        .iter()
        .filter(|i| i.name.eq_ignore_ascii_case(app) || i.resource_id.eq_ignore_ascii_case(app))
        .collect();
    let inst = match matches.as_slice() {
        [one] => *one,
        [] => {
            writeln!(out, "[FAIL] no instance named {app}")?;
            return Ok(false);
        }
        many => {
            writeln!(
                out,
                "[FAIL] {} instances named {app}; use the resource ID:",
                many.len()
            )?;
            list(out, many)?;
            return Ok(false);
        }
    };

    let u = refresh_dashboard(backend, inst, range, chrono::Utc::now()).await;
    for (source, e) in &u.errors {
        ok = false;
        writeln!(out, "[FAIL] {}: {e}", source_name(*source))?;
    }
    for spec in COUNT_METRICS.iter().chain(AVERAGE_METRICS) {
        let series = u.series.get(&spec.series);
        let points = series.map_or(0, |s| s.points.iter().filter(|(_, v)| v.is_some()).count());
        match u.totals.get(&spec.series) {
            Some(total) => {
                let label = if COUNT_METRICS.contains(spec) {
                    "total"
                } else {
                    "avg"
                };
                writeln!(
                    out,
                    "[ok] {} {label}={} points={points}",
                    spec.metric,
                    num(*total)
                )?;
            }
            None => writeln!(out, "[--] {} no data", spec.metric)?,
        }
    }
    if let Some(w) = &u.alert_window {
        writeln!(
            out,
            "[ok] alert window: failed_5m={} exceptions_5m={} dep_failures_5m={}",
            num(w.failed_5m),
            num(w.server_exceptions_5m),
            num(w.dependency_failures_5m)
        )?;
    }
    if let Some(s) = &u.scalars {
        let f = |v: Option<f64>| v.map(num).unwrap_or_else(|| "—".into());
        writeln!(
            out,
            "[ok] summary users_5m={} users={} sessions={} servers={} p95={}ms",
            f(s.users_5m),
            f(s.users_range),
            f(s.sessions_range),
            f(s.servers_10m),
            f(s.response_p95_ms)
        )?;
    }
    if let Some(s) = u.series.get(&crate::data::catalog::SeriesId::Sessions) {
        writeln!(out, "[ok] users/sessions series: {} rows", s.points.len())?;
    }
    if let Some(t) = &u.tops {
        writeln!(
            out,
            "[ok] failure tops (15m): {} endpoints, {} exceptions",
            t.endpoints.len(),
            t.exceptions.len()
        )?;
    }
    if !drill_walk(backend, inst, range, out).await? {
        ok = false;
    }
    Ok(ok)
}

/// Where the probe goes next from a loaded screen: the first row that opens
/// something, so every query on the path is run once against real data.
fn first_next(data: &ScreenData) -> Option<DrillTarget> {
    match data {
        ScreenData::List(l) => l.rows.iter().find_map(|r| r.next.clone()),
        ScreenData::Endpoint(e) => e
            .recent
            .first()
            .map(|r| DrillTarget::RequestDetail {
                operation_id: r.operation_id.clone(),
                request_id: Some(r.request_id.clone()).filter(|id| !id.is_empty()),
            })
            .or_else(|| {
                e.exceptions
                    .first()
                    .map(|x| DrillTarget::ExceptionOccurrences {
                        browser: false,
                        exception_type: x.name.clone(),
                    })
            }),
        ScreenData::Request(_) => None,
    }
}

/// Loads every drill-down screen reachable from the dashboard (following
/// the first row down each path), so KQL errors surface here instead of in
/// the UI. Prints only screen names and row counts.
async fn drill_walk(
    backend: &dyn Backend,
    inst: &AppInstance,
    range: TimeRange,
    out: &mut dyn Write,
) -> std::io::Result<bool> {
    let roots = [
        DrillTarget::FailingEndpoints,
        DrillTarget::ExceptionTypes { browser: false },
        DrillTarget::ExceptionTypes { browser: true },
        DrillTarget::FailingDependencies,
        DrillTarget::SlowEndpoints,
        DrillTarget::SlowPages,
        DrillTarget::AvailabilityTests,
        DrillTarget::InstanceBreakdown {
            metric: InstanceMetric::Cpu,
        },
        DrillTarget::InstanceBreakdown {
            metric: InstanceMetric::Memory,
        },
    ];
    let mut ok = true;
    for root in roots {
        let mut target = Some(root);
        let mut depth = 0;
        while let Some(t) = target.take() {
            match load(backend, inst, range, &t, chrono::Utc::now()).await {
                Ok(data) => {
                    writeln!(out, "[ok] drill {}: {} rows", t.crumb(), row_count(&data))?;
                    if depth < 3 {
                        target = first_next(&data);
                    }
                }
                Err(e) => {
                    ok = false;
                    writeln!(out, "[FAIL] drill {}: {e}", t.crumb())?;
                }
            }
            depth += 1;
        }
    }
    Ok(ok)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::azure::auth::StaticTokenSource;
    use crate::azure::http::AzError;
    use crate::azure::metrics::Aggregation;
    use crate::data::backend::fake::FakeBackend;
    use crate::data::demo::{DemoBackend, demo_instances};

    async fn run(backend: &dyn Backend, app: Option<&str>) -> (bool, String) {
        let mut out = Vec::new();
        let ok = run_probe(
            &StaticTokenSource("t".into()),
            backend,
            app,
            TimeRange::H4,
            &mut out,
        )
        .await
        .unwrap();
        (ok, String::from_utf8(out).unwrap())
    }

    #[tokio::test]
    async fn probe_with_demo_backend_is_ok() {
        let (ok, out) = run(&DemoBackend::new(), Some("demo-app-prod")).await;
        assert!(ok, "{out}");
        assert!(out.contains("[ok] token arm"));
        assert!(out.contains("[ok] discovery: 2 instances"));
        assert!(out.contains("[ok] requests/failed total=24"));
        assert!(out.contains("[--] performanceCounters/processorCpuPercentage no data"));
        assert!(out.contains("[ok] summary users_5m=165"));
        assert!(!out.contains("[FAIL]"));
        assert!(!out.contains("00000000-0000"), "never prints IDs: {out}");
        assert!(out.contains("[ok] drill failed requests:"), "{out}");
        assert!(
            out.contains("[ok] drill request:"),
            "walks down to a request: {out}"
        );
        assert!(out.contains("[ok] drill memory per instance:"), "{out}");
    }

    #[tokio::test]
    async fn probe_reports_failures() {
        let b = FakeBackend::new()
            .with_instances(demo_instances())
            .with_metrics(|req| {
                if req.aggregation == Aggregation::Average {
                    Err(AzError::Http {
                        status: 403,
                        message: "denied".into(),
                    })
                } else {
                    Ok(vec![])
                }
            });
        let (ok, out) = run(&b, Some("DEMO-APP-PROD")).await;
        assert!(!ok);
        assert!(
            out.contains("[FAIL] average metrics: HTTP 403: denied"),
            "{out}"
        );
    }

    #[tokio::test]
    async fn probe_without_app_lists_instances() {
        let (ok, out) = run(&DemoBackend::new(), None).await;
        assert!(ok);
        assert!(out.contains("demo-app-staging"));
    }

    #[tokio::test]
    async fn probe_unknown_app_fails() {
        let (ok, out) = run(&DemoBackend::new(), Some("nope")).await;
        assert!(!ok);
        assert!(out.contains("[FAIL] no instance named nope"));
    }
}
