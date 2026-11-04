//! The Overview tab: headline tiles and the 3×3 grid of charts.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Line;

use crate::app::PanelId;
use crate::data::catalog::SeriesId;
use crate::data::format::{fmt_big, fmt_bytes, fmt_ms, fmt_num, fmt_rate_bytes};
use crate::ui::common::{ChartSpec, Ctx, chart_panel, put_line};
use crate::ui::layout::OverviewLayout;
use crate::ui::theme::{Gradient, Rgb, palette};
use crate::ui::widgets::bigdigits::big_text;
use crate::ui::widgets::panel::{Emphasis, PanelBlock};

struct Kpi {
    title: &'static str,
    value: Option<f64>,
    label: String,
    color: Rgb,
    alert: bool,
}

fn kpis(ctx: &Ctx) -> Vec<Kpi> {
    let app = ctx.app;
    let scalars = app.store.scalars.as_ref().map(|s| &s.value);
    let range = app.range.label();
    let failed = app.store.alert_window.as_ref().map(|w| w.value.failed_5m);
    let alerting = app.alerts.failures();
    let availability = ctx.total(SeriesId::Availability);
    vec![
        Kpi {
            title: "users",
            value: scalars.and_then(|s| s.users_5m),
            label: "active · 5m".into(),
            color: palette::CYAN,
            alert: false,
        },
        Kpi {
            title: "sessions",
            value: scalars.and_then(|s| s.sessions_range),
            label: format!("unique · {range}"),
            color: palette::CYAN,
            alert: false,
        },
        Kpi {
            title: "requests",
            value: ctx
                .total(SeriesId::RequestsCount)
                .map(|n| n / app.range.minutes() as f64),
            label: "per min · avg".into(),
            color: palette::BLUE,
            alert: false,
        },
        Kpi {
            title: "failed",
            value: failed,
            label: if alerting { "▲ last 5m" } else { "last 5m" }.into(),
            color: if alerting {
                palette::RED
            } else if failed == Some(0.0) {
                palette::GREEN
            } else {
                palette::YELLOW
            },
            alert: alerting,
        },
        Kpi {
            title: "exceptions",
            value: ctx.total(SeriesId::ExceptionsServer),
            label: format!("server · {range}"),
            color: palette::PURPLE,
            alert: false,
        },
        Kpi {
            title: "response",
            // Seconds once it reaches 1 s, so the tile never reads "1.5 k ms".
            value: ctx
                .total(SeriesId::ServerResponse)
                .map(|ms| if ms >= 1000.0 { ms / 1000.0 } else { ms }),
            label: if ctx
                .total(SeriesId::ServerResponse)
                .is_some_and(|ms| ms >= 1000.0)
            {
                "s avg".into()
            } else {
                "ms avg".into()
            },
            color: palette::YELLOW,
            alert: false,
        },
        Kpi {
            title: "availability",
            value: availability,
            label: if availability.is_some() {
                format!("% · {range}")
            } else {
                "no tests".into()
            },
            color: palette::GREEN,
            alert: false,
        },
    ]
}

fn kpi_tile(buf: &mut Buffer, area: Rect, ctx: &Ctx, k: &Kpi) {
    PanelBlock {
        title: k.title,
        hotkey: None,
        right: None,
        emphasis: if k.alert {
            Emphasis::Alert
        } else {
            Emphasis::Normal
        },
        stale: None,
        theme: ctx.theme,
    }
    .render(area, buf);
    let inner = PanelBlock::inner(area);
    let (digits, unit) = match k.value {
        Some(v) => fmt_big(v),
        None => ("-".into(), ""),
    };
    for (i, row) in big_text(&digits).iter().enumerate() {
        put_line(
            buf,
            inner,
            i as u16,
            &Line::from(ctx.s(row.clone(), k.color)),
        );
    }
    let label = if unit.is_empty() {
        k.label.clone()
    } else {
        format!("{unit} {}", k.label)
    };
    put_line(buf, inner, 3, &Line::from(ctx.s(label, palette::DIM)));
}

fn kpi_strip(buf: &mut Buffer, area: Rect, ctx: &Ctx, all: &[Kpi]) {
    let mut spans = Vec::new();
    for (i, k) in all.iter().enumerate() {
        if i > 0 {
            spans.push(ctx.s(" │ ", palette::DIM));
        } else {
            spans.push(ctx.s(" ", palette::DIM));
        }
        let short = match k.title {
            "requests" => "req/min",
            "response" => "resp",
            "availability" => "avail",
            other => other,
        };
        spans.push(ctx.s(format!("{short} "), palette::SOFT));
        let v = match (k.title, k.value) {
            (_, None) => "—".to_string(),
            ("response", Some(_)) => ctx
                .total(SeriesId::ServerResponse)
                .map(fmt_ms)
                .unwrap_or_else(|| "—".into()),
            ("availability", Some(v)) => format!("{}%", fmt_num(v)),
            (_, Some(v)) => fmt_num(v),
        };
        spans.push(ctx.s(v, k.color));
    }
    put_line(buf, area, 0, &Line::from(spans));
}

fn legend(ctx: &Ctx, parts: Vec<(&'static str, Rgb, String)>) -> Line<'static> {
    let mut spans = vec![ctx.s(" ", palette::SOFT)];
    for (label, color, value) in parts {
        spans.push(ctx.s(format!("{label} "), color));
        spans.push(ctx.s(format!("{value}  "), palette::TEXT));
    }
    Line::from(spans)
}

fn dim(ctx: &Ctx, s: impl Into<String>) -> Option<Line<'static>> {
    Some(Line::from(ctx.s(s, palette::DIM)))
}

/// The nine Overview panels, in hotkey order.
pub(crate) fn panel_specs<'a>(ctx: &Ctx) -> Vec<ChartSpec<'a>> {
    let app = ctx.app;
    let scalars = app.store.scalars.as_ref().map(|s| &s.value);
    let range = app.range.label();
    let failed_5m = app.store.alert_window.as_ref().map(|w| w.value.failed_5m);
    let failed_right = match (app.alerts.failures(), failed_5m) {
        (true, Some(n)) => Some(Line::from(ctx.s(format!("▲ {n:.0} in 5m"), palette::RED))),
        (_, Some(n)) => dim(ctx, format!("5m: {n:.0}")),
        _ => None,
    };
    let count = |id| ctx.fmt_total(id, fmt_num);
    let ms = |id| ctx.fmt_total(id, fmt_ms);
    let pct = |id| ctx.fmt_total(id, |v| format!("{}%", fmt_num(v)));
    vec![
        ChartSpec {
            panel: PanelId::Sessions,
            title: "sessions & users",
            right: dim(ctx, range),
            top: (SeriesId::Sessions, Gradient::Cool),
            bottom: Some((SeriesId::Users, Gradient::Violet)),
            max: None,
            legend: legend(
                ctx,
                vec![
                    (
                        "▲ sessions",
                        palette::CYAN,
                        ctx.num(scalars.and_then(|s| s.sessions_range)),
                    ),
                    (
                        "▼ users",
                        palette::PURPLE,
                        ctx.num(scalars.and_then(|s| s.users_range)),
                    ),
                ],
            ),
        },
        ChartSpec {
            panel: PanelId::FailedRequests,
            title: "failed requests",
            right: failed_right,
            top: (SeriesId::RequestsFailed, Gradient::Hot),
            bottom: None,
            max: None,
            legend: legend(
                ctx,
                vec![(
                    "■ failed",
                    palette::RED,
                    format!("{} · {range}", count(SeriesId::RequestsFailed)),
                )],
            ),
        },
        ChartSpec {
            panel: PanelId::ServerResponse,
            title: "server response",
            right: dim(ctx, "avg"),
            top: (SeriesId::ServerResponse, Gradient::Teal),
            bottom: None,
            max: None,
            legend: legend(
                ctx,
                vec![
                    ("■ avg", palette::CYAN, ms(SeriesId::ServerResponse)),
                    (
                        "p95",
                        palette::DIM,
                        scalars
                            .and_then(|s| s.response_p95_ms)
                            .map(fmt_ms)
                            .unwrap_or_else(|| "—".into()),
                    ),
                ],
            ),
        },
        ChartSpec {
            panel: PanelId::PageLoad,
            title: "page load",
            right: dim(ctx, "browser"),
            top: (SeriesId::PageLoadTotal, Gradient::Lime),
            bottom: None,
            max: None,
            legend: legend(
                ctx,
                vec![
                    ("client", palette::GREEN, ms(SeriesId::PageLoadProcessing)),
                    ("network", palette::GREEN, ms(SeriesId::PageLoadNetwork)),
                    ("send", palette::GREEN, ms(SeriesId::PageLoadSend)),
                    ("receive", palette::GREEN, ms(SeriesId::PageLoadReceive)),
                ],
            ),
        },
        ChartSpec {
            panel: PanelId::ExceptionsDeps,
            title: "exceptions / dependencies",
            right: dim(ctx, "count"),
            top: (SeriesId::ExceptionsServer, Gradient::Violet),
            bottom: Some((SeriesId::DependenciesFailed, Gradient::Hot)),
            max: None,
            legend: legend(
                ctx,
                vec![
                    (
                        "▲ exceptions",
                        palette::PURPLE,
                        count(SeriesId::ExceptionsServer),
                    ),
                    (
                        "▼ dep. failures",
                        palette::ORANGE,
                        count(SeriesId::DependenciesFailed),
                    ),
                ],
            ),
        },
        ChartSpec {
            panel: PanelId::Cpu,
            title: "cpu",
            right: dim(ctx, "process"),
            top: (SeriesId::ProcessCpu, Gradient::Load),
            bottom: None,
            max: Some(100.0),
            legend: legend(
                ctx,
                vec![
                    ("process", palette::GREEN, pct(SeriesId::ProcessCpu)),
                    ("processor", palette::GREEN, pct(SeriesId::ProcessorCpu)),
                ],
            ),
        },
        ChartSpec {
            panel: PanelId::Availability,
            title: "availability",
            right: dim(ctx, range),
            top: (SeriesId::Availability, Gradient::Ok),
            bottom: None,
            max: Some(100.0),
            legend: legend(
                ctx,
                vec![
                    ("avail", palette::GREEN, pct(SeriesId::Availability)),
                    ("tests", palette::GREEN, count(SeriesId::AvailabilityCount)),
                ],
            ),
        },
        ChartSpec {
            panel: PanelId::BrowserExceptions,
            title: "browser exceptions",
            right: dim(ctx, "count"),
            top: (SeriesId::ExceptionsBrowser, Gradient::Violet),
            bottom: None,
            max: None,
            legend: legend(
                ctx,
                vec![("count", palette::PURPLE, count(SeriesId::ExceptionsBrowser))],
            ),
        },
        ChartSpec {
            panel: PanelId::MemoryIo,
            title: "memory & i/o",
            right: dim(ctx, "available"),
            top: (SeriesId::MemoryAvailable, Gradient::Lime),
            bottom: None,
            max: None,
            legend: legend(
                ctx,
                vec![
                    (
                        "available",
                        palette::GREEN,
                        ctx.fmt_total(SeriesId::MemoryAvailable, fmt_bytes),
                    ),
                    (
                        "io",
                        palette::GREEN,
                        ctx.fmt_total(SeriesId::ProcessIo, fmt_rate_bytes),
                    ),
                ],
            ),
        },
    ]
}

pub fn render(buf: &mut Buffer, l: &OverviewLayout, ctx: &Ctx) {
    let all = kpis(ctx);
    if let Some(strip) = l.kpi_strip {
        kpi_strip(buf, strip, ctx, &all);
    }
    for (rect, k) in l.kpis.iter().zip(&all) {
        kpi_tile(buf, *rect, ctx, k);
    }
    for (rect, spec) in l.panels.iter().zip(panel_specs(ctx)) {
        chart_panel(buf, *rect, ctx, spec);
    }
}
