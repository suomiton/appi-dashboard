//! The Failures tab: the big failures panel, health meters and smaller panels.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::Widget;

use crate::app::{PanelId, hotkey};
use crate::data::catalog::SeriesId;
use crate::data::dashboard::Source;
use crate::data::format::{fmt_bytes, fmt_ms, fmt_num, truncate_width};
use crate::ui::common::{ChartSpec, Ctx, chart_body, chart_panel, put_line};
use crate::ui::layout::FailuresLayout;
use crate::ui::overview::panel_specs;
use crate::ui::theme::{Gradient, Rgb, palette};
use crate::ui::widgets::meter::Meter;
use crate::ui::widgets::panel::PanelBlock;

const SIDE_WIDTH: u16 = 38;

fn main_panel(buf: &mut Buffer, area: Rect, ctx: &Ctx) {
    let app = ctx.app;
    let failed = app.store.alert_window.as_ref().map(|w| w.value.failed_5m);
    let alert_stale = ctx.stale_timed(app.store.alert_window.as_ref());
    let right = match (alert_stale.is_some(), app.alerts.failures(), failed) {
        (true, _, Some(n)) => {
            Line::from(ctx.s(format!("{n:.0} failed · last known"), palette::DIM))
        }
        (_, true, Some(n)) => {
            Line::from(ctx.s(format!("▲ {n:.0} failed requests in last 5m"), palette::RED))
        }
        (_, false, Some(0.0)) => Line::from(ctx.s("no failures in last 5m", palette::GREEN)),
        (_, false, Some(n)) => {
            Line::from(ctx.s(format!("{n:.0} failed in last 5m"), palette::YELLOW))
        }
        (_, _, None) => Line::from(ctx.s("—", palette::DIM)),
    };
    PanelBlock {
        title: "failures",
        hotkey: hotkey(app.view, PanelId::FailuresMain),
        right: Some(right),
        emphasis: ctx.emphasis(PanelId::FailuresMain),
        stale: alert_stale.or_else(|| ctx.stale(SeriesId::RequestsFailed)),
        theme: ctx.theme,
    }
    .render(area, buf);
    let inner = PanelBlock::inner(area).intersection(buf.area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let (chart_area, side) = if inner.width >= 70 {
        let side_x = inner.x + inner.width - SIDE_WIDTH;
        (
            Rect::new(inner.x, inner.y, inner.width - SIDE_WIDTH - 1, inner.height),
            Some(Rect::new(side_x, inner.y, SIDE_WIDTH, inner.height)),
        )
    } else {
        (inner, None)
    };
    let range = app.range.label();
    let count = |id| ctx.fmt_total(id, fmt_num);
    let legend = Line::from(vec![
        ctx.s(" ▲ failed requests ", palette::RED),
        ctx.s(
            format!("{}  ", count(SeriesId::RequestsFailed)),
            palette::TEXT,
        ),
        ctx.s("▼ server exceptions ", palette::PURPLE),
        ctx.s(
            format!("{}  ", count(SeriesId::ExceptionsServer)),
            palette::TEXT,
        ),
        ctx.s("■ dep. failures ", palette::ORANGE),
        ctx.s(
            format!("{} · {range}", count(SeriesId::DependenciesFailed)),
            palette::TEXT,
        ),
    ]);
    chart_body(
        buf,
        chart_area,
        ctx,
        (SeriesId::RequestsFailed, Gradient::Hot),
        Some((SeriesId::ExceptionsServer, Gradient::Violet)),
        None,
        &legend,
    );
    if let Some(side) = side {
        for y in 0..side.height {
            buf.set_string(side.x - 1, side.y + y, "│", ctx.theme.fg(palette::BORDER));
        }
        top_lists(
            buf,
            Rect::new(side.x + 1, side.y, side.width - 1, side.height),
            ctx,
        );
    }
}

fn top_lists(buf: &mut Buffer, area: Rect, ctx: &Ctx) {
    let w = area.width as usize;
    let mut lines: Vec<Line> = Vec::new();
    let row = |n: f64, name: &str, color: Rgb| {
        let n = format!("{:>3} ", fmt_num(n));
        let name = truncate_width(name, w.saturating_sub(n.chars().count()));
        Line::from(vec![ctx.s(n, color), ctx.s(name, palette::TEXT)])
    };
    match ctx.app.store.tops.as_ref().map(|t| &t.value) {
        None => match ctx.app.store.source_errors.get(&Source::Tops) {
            Some(err) => lines.push(Line::from(ctx.s(format!("⚠ {err}"), palette::RED))),
            None => lines.push(Line::from(ctx.s("loading…", palette::DIM))),
        },
        Some(tops) => {
            let age = ctx.app.store.tops.as_ref().map(|t| ctx.now - t.at);
            let limit = chrono::Duration::seconds(ctx.app.refresh_secs as i64 + 60);
            let mut heading = vec![ctx.s("top failing endpoints · 15m", palette::DIM)];
            if let Some(age) = age.filter(|a| *a > limit) {
                heading.push(ctx.s(
                    format!(" · {} old", crate::data::format::fmt_age(age)),
                    palette::YELLOW,
                ));
            }
            lines.push(Line::from(heading));
            if tops.endpoints.is_empty() {
                lines.push(Line::from(ctx.s("none", palette::GREEN)));
            }
            for (i, e) in tops.endpoints.iter().take(5).enumerate() {
                let color = if i == 0 {
                    palette::RED
                } else {
                    palette::YELLOW
                };
                lines.push(row(e.n, &e.name, color));
            }
            lines.push(Line::default());
            lines.push(Line::from(ctx.s("top exceptions · 15m", palette::DIM)));
            if tops.exceptions.is_empty() {
                lines.push(Line::from(ctx.s("none", palette::GREEN)));
            }
            for e in tops.exceptions.iter().take(5) {
                lines.push(row(e.n, &e.name, palette::PURPLE));
            }
        }
    }
    for (i, line) in lines.iter().enumerate() {
        if (i as u16) + 1 < area.height {
            put_line(buf, area, i as u16, line);
        }
    }
    if area.height > 0 {
        put_line(
            buf,
            area,
            area.height - 1,
            &Line::from(ctx.s("↵ drill in", palette::DIM)),
        );
    }
}

fn health_panel(buf: &mut Buffer, area: Rect, ctx: &Ctx) {
    let app = ctx.app;
    PanelBlock {
        title: "health",
        hotkey: hotkey(app.view, PanelId::Health),
        right: None,
        emphasis: ctx.emphasis(PanelId::Health),
        stale: ctx
            .stale_timed(app.store.scalars.as_ref())
            .or_else(|| ctx.stale_timed(app.store.alert_window.as_ref())),
        theme: ctx.theme,
    }
    .render(area, buf);
    let inner = PanelBlock::inner(area).intersection(buf.area);
    if inner.width < 20 || inner.height == 0 {
        return;
    }
    let scalars = app.store.scalars.as_ref().map(|s| &s.value);
    let availability = ctx.total(SeriesId::Availability);
    let avg = ctx.total(SeriesId::ServerResponse);
    let p95 = scalars.and_then(|s| s.response_p95_ms);
    let cpu = ctx.total(SeriesId::ProcessCpu);
    let memory = app
        .store
        .series
        .get(&SeriesId::MemoryAvailable)
        .map(|s| &s.value);
    let mem_latest = memory.and_then(|m| m.latest());
    let response_threshold = app.config.alerts.response_avg_above_ms.max(1.0);
    let dep_5m = app
        .store
        .alert_window
        .as_ref()
        .map(|w| w.value.dependency_failures_5m);
    let threshold = app.config.alerts.dependency_failures_5m.max(1.0);
    let pct = |v: Option<f64>| {
        v.map(|v| format!("{}%", fmt_num(v)))
            .unwrap_or_else(|| "—".into())
    };
    let rows: Vec<(&str, Option<f64>, Gradient, String, Rgb)> = vec![
        (
            "availability",
            availability.map(|a| a / 100.0),
            Gradient::Ok,
            pct(availability),
            palette::GREEN,
        ),
        (
            "response avg",
            avg.map(|v| (v / response_threshold).min(1.0)),
            Gradient::Load,
            avg.map(fmt_ms).unwrap_or_else(|| "—".into()),
            palette::TEXT,
        ),
        (
            "response p95",
            p95.map(|v| (v / response_threshold).min(1.0)),
            Gradient::Load,
            p95.map(fmt_ms).unwrap_or_else(|| "—".into()),
            palette::YELLOW,
        ),
        (
            "cpu",
            cpu.map(|v| v / 100.0),
            Gradient::Load,
            pct(cpu),
            palette::TEXT,
        ),
        (
            "memory free",
            None,
            Gradient::Lime,
            mem_latest.map(fmt_bytes).unwrap_or_else(|| "—".into()),
            palette::TEXT,
        ),
        (
            "dep. fails 5m",
            dep_5m.map(|n| (n / threshold).min(1.0)),
            Gradient::Hot,
            ctx.num(dep_5m),
            palette::ORANGE,
        ),
        (
            "avail. tests",
            availability.map(|a| a / 100.0),
            Gradient::Ok,
            ctx.fmt_total(SeriesId::AvailabilityCount, fmt_num),
            palette::GREEN,
        ),
    ];
    let value_w = 9u16;
    // " " + 13-character label + gap
    let label_w = 15u16;
    let meter_w = inner.width.saturating_sub(label_w + value_w + 1).min(14);
    for (i, (label, ratio, grad, value, color)) in rows.into_iter().enumerate() {
        let y = i as u16;
        if y >= inner.height {
            break;
        }
        put_line(
            buf,
            inner,
            y,
            &Line::from(ctx.s(format!(" {label}"), palette::SOFT)),
        );
        if meter_w > 0
            && let Some(ratio) = ratio
        {
            Meter {
                ratio,
                gradient: grad,
                theme: ctx.theme,
            }
            .render(Rect::new(inner.x + label_w, inner.y + y, meter_w, 1), buf);
        }
        let v = truncate_width(&value, value_w as usize);
        let vx =
            inner.x + inner.width - 1 - unicode_width::UnicodeWidthStr::width(v.as_str()) as u16;
        buf.set_string(vx, inner.y + y, v, ctx.theme.fg(color));
    }
    let requests = ctx
        .total(SeriesId::RequestsCount)
        .map(|n| n / app.range.minutes() as f64);
    let extra = [
        Line::from(vec![
            ctx.s(" sessions ", palette::SOFT),
            ctx.s(
                ctx.num(scalars.and_then(|s| s.sessions_range)),
                palette::CYAN,
            ),
            ctx.s("  users ", palette::SOFT),
            ctx.s(
                ctx.num(scalars.and_then(|s| s.users_range)),
                palette::PURPLE,
            ),
        ]),
        Line::from(vec![
            ctx.s(" requests ", palette::SOFT),
            ctx.s(format!("{}/min", ctx.num(requests)), palette::BLUE),
            ctx.s("  servers ", palette::SOFT),
            ctx.s(ctx.num(scalars.and_then(|s| s.servers_10m)), palette::GREEN),
        ]),
    ];
    for (i, line) in extra.iter().enumerate() {
        put_line(buf, inner, 8 + i as u16, line);
    }
}

/// The eight smaller Failures-tab panels, in hotkey order (3–9, 0).
fn small_specs<'a>(ctx: &Ctx) -> Vec<ChartSpec<'a>> {
    let mut o: Vec<Option<ChartSpec>> = panel_specs(ctx).into_iter().map(Some).collect();
    let mut take = |p: PanelId| {
        o.iter_mut()
            .find(|s| s.as_ref().is_some_and(|s| s.panel == p))
            .and_then(Option::take)
    };
    let mut traffic = take(PanelId::Sessions);
    if let Some(t) = traffic.as_mut() {
        t.title = "traffic";
    }
    let count = |id| ctx.fmt_total(id, fmt_num);
    let deps = ChartSpec {
        panel: PanelId::DependencyFailures,
        title: "dependency failures",
        right: Some(Line::from(ctx.s("count", palette::DIM))),
        top: (SeriesId::DependenciesFailed, Gradient::Hot),
        bottom: None,
        max: None,
        legend: Line::from(vec![
            ctx.s(" failed ", palette::ORANGE),
            ctx.s(count(SeriesId::DependenciesFailed), palette::TEXT),
        ]),
    };
    let tests = ChartSpec {
        panel: PanelId::Availability,
        title: "availability tests",
        right: Some(Line::from(ctx.s(ctx.app.range.label(), palette::DIM))),
        top: (SeriesId::AvailabilityCount, Gradient::Teal),
        bottom: None,
        max: None,
        legend: Line::from(vec![
            ctx.s(" runs ", palette::CYAN),
            ctx.s(
                format!("{}  ", count(SeriesId::AvailabilityCount)),
                palette::TEXT,
            ),
            ctx.s("avail ", palette::GREEN),
            ctx.s(
                ctx.fmt_total(SeriesId::Availability, |v| format!("{}%", fmt_num(v))),
                palette::TEXT,
            ),
        ]),
    };
    [
        traffic,
        take(PanelId::ServerResponse),
        take(PanelId::PageLoad),
        take(PanelId::BrowserExceptions),
        take(PanelId::Cpu),
        take(PanelId::MemoryIo),
        Some(deps),
        Some(tests),
    ]
    .into_iter()
    .flatten()
    .collect()
}

pub fn render(buf: &mut Buffer, l: &FailuresLayout, ctx: &Ctx) {
    main_panel(buf, l.main, ctx);
    health_panel(buf, l.health, ctx);
    for (rect, spec) in l.panels.iter().zip(small_specs(ctx)) {
        chart_panel(buf, *rect, ctx, spec);
    }
}
