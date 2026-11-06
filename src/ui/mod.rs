//! Terminal UI: theme, widgets, layouts and screens.

pub mod common;
pub mod drill;
pub mod failures;
pub mod layout;
pub mod overview;
pub mod picker;
pub mod theme;
pub mod widgets;

use std::time::Instant;

use chrono::{DateTime, FixedOffset, Utc};
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Widget};

use crate::app::{App, View};
use crate::azure::auth::AuthError;
use crate::ui::common::{Ctx, put_centered, put_line};
use crate::ui::layout::{MIN_HEIGHT, MIN_WIDTH, centered};
use crate::ui::theme::{Theme, palette};
use crate::ui::widgets::panel::{Emphasis, PanelBlock};

pub use layout::{failures_layout, overview_layout};

/// Draws the whole screen. `now` is the wall clock in the local offset;
/// `mono` is the monotonic clock the refresh countdown is measured on.
pub fn draw(f: &mut Frame, app: &App, theme: &Theme, now: DateTime<FixedOffset>, mono: Instant) {
    let area = f.area();
    render(f.buffer_mut(), area, app, theme, now, mono);
}

pub fn render(
    buf: &mut Buffer,
    area: Rect,
    app: &App,
    theme: &Theme,
    now: DateTime<FixedOffset>,
    mono: Instant,
) {
    Block::new().style(theme.base()).render(area, buf);
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        put_centered(
            buf,
            area,
            &format!(
                "Terminal too small: need {MIN_WIDTH}×{MIN_HEIGHT}, have {}×{}",
                area.width, area.height
            ),
            theme.fg(palette::YELLOW),
        );
        return;
    }
    let ctx = Ctx {
        app,
        theme,
        now: now.with_timezone(&Utc),
        offset: *now.offset(),
    };
    if let Some(problem) = &app.auth_problem {
        let rows = Rect::new(area.x, area.y, area.width, 1);
        header(buf, rows, &ctx, now, mono);
        auth_screen(buf, area, &ctx, problem);
        if let Some(p) = &app.picker {
            picker::render(buf, area, &ctx, p);
        }
        if app.toast.is_some() {
            footer(
                buf,
                Rect::new(area.x, area.y + area.height - 1, area.width, 1),
                &ctx,
            );
        }
        return;
    }
    if app.instance.is_none() {
        header(
            buf,
            Rect::new(area.x, area.y, area.width, 1),
            &ctx,
            now,
            mono,
        );
        if let Some(p) = &app.picker {
            picker::render(buf, area, &ctx, p);
        }
        if app.toast.is_some() {
            footer(
                buf,
                Rect::new(area.x, area.y + area.height - 1, area.width, 1),
                &ctx,
            );
        }
        return;
    }
    if !app.drill.is_empty() {
        let rows = Rect::new(area.x, area.y, area.width, 1);
        header(buf, rows, &ctx, now, mono);
        drill::render(
            buf,
            Rect::new(area.x, area.y + 1, area.width, area.height - 1),
            &ctx,
        );
    } else {
        match app.view {
            View::Overview => {
                let l = overview_layout(area);
                header(buf, l.header, &ctx, now, mono);
                overview::render(buf, &l, &ctx);
                footer(buf, l.footer, &ctx);
            }
            View::Failures => {
                let l = failures_layout(area);
                header(buf, l.header, &ctx, now, mono);
                failures::render(buf, &l, &ctx);
                footer(buf, l.footer, &ctx);
            }
        }
    }
    if let Some(p) = &app.picker {
        picker::render(buf, area, &ctx, p);
    } else if app.help {
        help(buf, area, &ctx);
    }
}

fn header(buf: &mut Buffer, area: Rect, ctx: &Ctx, now: DateTime<FixedOffset>, mono: Instant) {
    let app = ctx.app;
    let t = ctx.theme;
    let throttled = app.is_throttled(mono);
    let sep = || ctx.s(" │ ", palette::DIM);
    let mut left: Vec<Span> = Vec::new();
    let name = app
        .instance
        .as_ref()
        .map(|i| i.name.clone())
        .unwrap_or_else(|| "no instance".into());
    // Keep the tab bar (and the failures pulse) on screen on narrow terminals.
    let name = crate::data::format::truncate_width(&name, (area.width / 5).max(12) as usize);
    left.push(ctx.s(format!(" ◆ {name} "), palette::BLUE));
    if let Some(label) = app
        .instance
        .as_ref()
        .and_then(|i| app.config.label_for(&i.resource_id))
    {
        left.push(Span::styled(
            format!(" {label} "),
            ratatui::style::Style::new()
                .fg(t.color(palette::BG))
                .bg(t.color(palette::RED))
                .add_modifier(Modifier::BOLD),
        ));
    }
    left.push(sep());
    let active = |on: bool| {
        if on {
            ratatui::style::Style::new()
                .fg(t.color(palette::BG))
                .bg(t.color(palette::BLUE))
                .add_modifier(Modifier::BOLD)
        } else {
            t.fg(palette::DIM)
        }
    };
    left.push(Span::styled(
        " overview ",
        active(app.view == View::Overview),
    ));
    left.push(ctx.s(" ", palette::DIM));
    let alerting = app.alerts.failures();
    let failures_style = match (alerting, app.view == View::Failures) {
        (true, true) => ratatui::style::Style::new()
            .fg(t.color(palette::BG))
            .bg(t.color(palette::RED))
            .add_modifier(Modifier::BOLD),
        (true, false) => {
            let pulse = if now.timestamp() % 2 == 0 {
                palette::RED
            } else {
                palette::DIM
            };
            t.fg(pulse).add_modifier(Modifier::BOLD)
        }
        (false, on) => active(on),
    };
    let failed = app
        .store
        .alert_window
        .as_ref()
        .map(|w| w.value.failed_5m)
        .unwrap_or(0.0);
    let label = if let Some(age) = ctx.stale_timed(app.store.alert_window.as_ref()) {
        format!(" failures · stale {age} ")
    } else if alerting {
        format!(" failures ● {failed:.0} ")
    } else {
        " failures ".to_string()
    };
    left.push(Span::styled(label, failures_style));
    left.push(sep());
    left.push(ctx.s("range ", palette::SOFT));
    left.push(ctx.s(app.range.label(), palette::TEXT));
    left.push(sep());
    left.push(ctx.s("refresh ", palette::SOFT));
    left.push(ctx.s(format!("{}s ", app.refresh_secs), palette::TEXT));
    if app.refreshing {
        left.push(ctx.s("refreshing…", palette::DIM));
    } else if !throttled && let Some(s) = app.seconds_to_refresh(mono) {
        left.push(ctx.s(format!("next {s}s ⟳"), palette::DIM));
    }
    if let Some(s) = app.store.scalars.as_ref().and_then(|s| s.value.servers_10m) {
        left.push(sep());
        if let Some(age) = ctx.stale_timed(app.store.scalars.as_ref()) {
            left.push(ctx.s(format!("● {s:.0} servers · stale {age}"), palette::DIM));
        } else {
            left.push(ctx.s(format!("● {s:.0} servers"), palette::GREEN));
        }
    }

    let mut right: Vec<Span> = Vec::new();
    if throttled {
        // Reserve the right side so the cooldown survives narrow headers.
        right.push(ctx.s(
            format!(
                "throttled · next in {}s  ",
                app.seconds_to_refresh(mono).unwrap_or(0)
            ),
            palette::YELLOW,
        ));
    } else if let Some(err) = app.store.current_error() {
        let max = (area.width / 3).max(20) as usize;
        right.push(ctx.s(
            format!("⚠ {}  ", crate::data::format::truncate_width(err, max)),
            palette::YELLOW,
        ));
    }
    if !throttled || area.width >= 120 {
        right.push(ctx.s(now.format("%H:%M:%S").to_string(), palette::CYAN));
    }
    if area.width >= 120 && !throttled {
        right.push(ctx.s(
            format!(" {} · UTC{} ", now.format("%a %-d %b"), now.format("%:z")),
            palette::DIM,
        ));
    } else {
        right.push(ctx.s(" ", palette::DIM));
    }
    let right_line = Line::from(right);
    let rw = right_line.width() as u16;
    let left_line = Line::from(left);
    if area.width > rw + 1 {
        buf.set_line(area.x, area.y, &left_line, area.width - rw - 1);
        buf.set_line(area.x + area.width - rw, area.y, &right_line, rw);
    } else {
        buf.set_line(area.x, area.y, &left_line, area.width);
    }
}

fn footer(buf: &mut Buffer, area: Rect, ctx: &Ctx) {
    if let Some((msg, _)) = &ctx.app.toast {
        put_line(
            buf,
            area,
            0,
            &Line::from(ctx.s(format!(" {msg}"), palette::YELLOW)),
        );
        return;
    }
    let keys: &[(&str, &str)] = &[
        ("tab", "switch view"),
        ("1-9", "select panel"),
        ("↵", "drill in"),
        ("esc", "back"),
        ("r", "refresh"),
        ("+/-", "interval"),
        ("t", "time range"),
        ("i", "instance"),
        ("?", "help"),
        ("q", "quit"),
    ];
    let mut spans = vec![ctx.s(" ", palette::DIM)];
    for (k, label) in keys {
        spans.push(ctx.s(*k, palette::ORANGE));
        spans.push(ctx.s(format!(" {label}  "), palette::DIM));
    }
    put_line(buf, area, 0, &Line::from(spans));
}

pub(crate) const HELP_KEYS: &[(&str, &str)] = &[
    ("Tab / Shift+Tab", "switch view: overview ↔ failures"),
    ("1–9, 0", "select panel by its number"),
    ("arrow keys", "move selection"),
    ("PgUp / PgDn", "page through drill lists"),
    ("Home / End", "first / last row in drill lists"),
    ("picker text", "append; Backspace removes last char"),
    ("picker editing", "no cursor movement or Ctrl+U / Ctrl+W"),
    ("Enter", "drill in"),
    ("Esc", "back"),
    ("r", "refresh now"),
    ("+ / -", "longer / shorter refresh interval"),
    ("t", "cycle time range"),
    ("i", "pick application insights instance"),
    ("c", "copy operation ID (request detail)"),
    ("o", "open in Azure portal"),
    ("s", "sort (lists)"),
    ("?", "this help"),
    ("q", "quit"),
];

fn help(buf: &mut Buffer, area: Rect, ctx: &Ctx) {
    let box_area = centered(area, 62, HELP_KEYS.len() as u16 + 4);
    Clear.render(box_area, buf);
    PanelBlock {
        title: "help",
        hotkey: None,
        right: Some(Line::from(ctx.s("esc to close", palette::DIM))),
        emphasis: Emphasis::Focused,
        stale: None,
        theme: ctx.theme,
    }
    .render(box_area, buf);
    let inner = PanelBlock::inner(box_area);
    for (i, (k, what)) in HELP_KEYS.iter().enumerate() {
        put_line(
            buf,
            inner,
            i as u16 + 1,
            &Line::from(vec![
                ctx.s(format!(" {k:<17}"), palette::ORANGE),
                ctx.s(*what, palette::TEXT),
            ]),
        );
    }
}

fn auth_screen(buf: &mut Buffer, area: Rect, ctx: &Ctx, problem: &AuthError) {
    let box_area = centered(area, 70, 9);
    PanelBlock {
        title: match problem {
            AuthError::NotLoggedIn(_) => "Azure CLI sign-in needed",
            AuthError::AzNotInstalled => "Azure CLI required",
            AuthError::Other(_) => "Azure CLI error",
        },
        hotkey: None,
        right: None,
        emphasis: Emphasis::Alert,
        stale: None,
        theme: ctx.theme,
    }
    .render(box_area, buf);
    let inner = PanelBlock::inner(box_area);
    let (first, second) = match problem {
        AuthError::AzNotInstalled => (
            "The Azure CLI (az) was not found.".to_string(),
            "Install the Azure CLI: https://aka.ms/azcli".to_string(),
        ),
        AuthError::NotLoggedIn(msg) => (
            msg.clone(),
            "Run \"az login\" in another terminal. Retrying automatically.".to_string(),
        ),
        AuthError::Other(msg) => (
            msg.clone(),
            "The Azure CLI returned an error; retrying automatically.".into(),
        ),
    };
    put_line(
        buf,
        inner,
        1,
        &Line::from(ctx.s(format!(" {first}"), palette::TEXT)),
    );
    put_line(
        buf,
        inner,
        3,
        &Line::from(ctx.s(format!(" {second}"), palette::YELLOW)),
    );
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::app::{App, DataEvent, PanelId, View};
    use crate::azure::auth::AuthError;
    use crate::config::Config;
    use crate::data::catalog::SeriesId;
    use crate::data::dashboard::{TopItem, refresh_dashboard};
    use crate::data::demo::{DemoBackend, demo_instances};
    use crate::data::time::TimeRange;
    use crate::ui::theme::{Depth, Theme, palette};
    use chrono::{DateTime, FixedOffset, TimeZone, Utc};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;
    use std::time::Instant;

    pub(crate) fn wall() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 4, 13, 6, 42).unwrap()
    }

    pub(crate) fn local() -> DateTime<FixedOffset> {
        wall().with_timezone(&FixedOffset::east_opt(2 * 3600).unwrap())
    }

    /// An app filled with demo data as of `wall()`, plus the Instant its
    /// refresh schedule is anchored to.
    pub(crate) fn demo_app(view: View) -> (App, Instant) {
        let inst = demo_instances().remove(0);
        let mut app = App::new(Config::default(), Some(inst.clone()), TimeRange::H4, 30);
        let mono = Instant::now();
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let u = rt.block_on(refresh_dashboard(
            &DemoBackend::at(wall()),
            &inst,
            TimeRange::H4,
            wall(),
        ));
        app.on_tick(mono, wall());
        app.on_data(DataEvent::Dashboard(u), mono, wall());
        app.view = view;
        (app, mono)
    }

    pub(crate) fn render(app: &App, mono: Instant, w: u16, h: u16) -> Terminal<TestBackend> {
        let theme = Theme::new(Depth::TrueColor);
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| draw(f, app, &theme, local(), mono)).unwrap();
        t
    }

    pub(crate) fn text(t: &Terminal<TestBackend>) -> String {
        let buf = t.backend().buffer();
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn overview_snapshots() {
        let (app, mono) = demo_app(View::Overview);
        insta::assert_snapshot!("overview_160x45", render(&app, mono, 160, 45).backend());
        insta::assert_snapshot!("overview_100x30", render(&app, mono, 100, 30).backend());
    }

    #[test]
    fn failures_snapshots() {
        let (app, mono) = demo_app(View::Failures);
        insta::assert_snapshot!("failures_160x45", render(&app, mono, 160, 45).backend());
        insta::assert_snapshot!("failures_100x30", render(&app, mono, 100, 30).backend());
    }

    #[test]
    fn state_snapshots() {
        let (mut app, mono) = demo_app(View::Overview);
        insta::assert_snapshot!("too_small_70x20", render(&app, mono, 70, 20).backend());
        app.help = true;
        insta::assert_snapshot!("help_100x30", render(&app, mono, 100, 30).backend());
        app.help = false;
        app.auth_problem = Some(AuthError::NotLoggedIn(
            "ERROR: Please run 'az login'".into(),
        ));
        insta::assert_snapshot!(
            "auth_required_100x30",
            render(&app, mono, 100, 30).backend()
        );
        app.auth_problem = Some(AuthError::AzNotInstalled);
        assert!(text(&render(&app, mono, 100, 30)).contains("https://aka.ms/azcli"));
    }

    fn top_left_fg(t: &Terminal<TestBackend>, r: Rect) -> ratatui::style::Color {
        t.backend().buffer()[(r.x, r.y)].fg
    }

    #[test]
    fn failed_panel_border_red_when_alert() {
        let (app, mono) = demo_app(View::Overview);
        assert!(app.alerts.failures());
        let t = render(&app, mono, 160, 45);
        let rect = layout::overview_layout(Rect::new(0, 0, 160, 45)).panels[1];
        assert_eq!(
            top_left_fg(&t, rect),
            Theme::new(Depth::TrueColor).color(palette::RED)
        );
    }

    #[test]
    fn focused_panel_border_blue() {
        let (mut app, mono) = demo_app(View::Overview);
        app.focus = Some(PanelId::ServerResponse);
        let t = render(&app, mono, 160, 45);
        let rect = layout::overview_layout(Rect::new(0, 0, 160, 45)).panels[2];
        assert_eq!(
            top_left_fg(&t, rect),
            Theme::new(Depth::TrueColor).color(palette::BLUE)
        );
    }

    #[test]
    fn failures_tab_label_red_when_alert() {
        let (app, mono) = demo_app(View::Overview);
        let t = render(&app, mono, 160, 45);
        let header = text(&t).lines().next().unwrap().to_string();
        let col = header.find("failures").expect("failures tab in header");
        let x = header[..col].chars().count() as u16;
        let red = Theme::new(Depth::TrueColor).color(palette::RED);
        let dim = Theme::new(Depth::TrueColor).color(palette::DIM);
        let fg = t.backend().buffer()[(x, 0)].fg;
        assert!(fg == red || fg == dim, "pulses between red and dim: {fg:?}");
        assert!(header.contains("● 12"), "{header}");
    }

    #[test]
    fn no_data_panel_says_so() {
        let (mut app, mono) = demo_app(View::Overview);
        for id in [
            SeriesId::PageLoadTotal,
            SeriesId::PageLoadNetwork,
            SeriesId::PageLoadProcessing,
            SeriesId::PageLoadSend,
            SeriesId::PageLoadReceive,
        ] {
            app.store.series.remove(&id);
            app.store.totals.remove(&id);
        }
        let out = text(&render(&app, mono, 160, 45));
        assert!(out.contains("no data in range"));
        assert!(out.contains("client —"), "legend shows dashes");
    }

    #[test]
    fn errors_are_visible_when_nothing_loaded() {
        use crate::azure::http::AzError;
        use crate::data::dashboard::{DashboardUpdate, Source};
        let inst = demo_instances().remove(0);
        let mut app = App::new(Config::default(), Some(inst), TimeRange::H4, 30);
        let mono = Instant::now();
        app.on_tick(mono, wall());
        let mut u = DashboardUpdate::default();
        for src in [
            Source::CountMetrics,
            Source::AverageMetrics,
            Source::CountTotals,
            Source::AverageTotals,
            Source::AlertWindow,
            Source::Scalars,
            Source::UsersSessions,
        ] {
            u.errors.push((
                src,
                AzError::Http {
                    status: 403,
                    message: "AuthorizationFailed: no access".into(),
                },
            ));
        }
        app.on_data(DataEvent::Dashboard(u), mono, wall());
        let out = text(&render(&app, mono, 160, 45));
        assert!(out.contains("HTTP 403"), "{out}");
        assert!(
            !out.contains("no data in range"),
            "errors are not shown as no data: {out}"
        );
    }

    #[test]
    fn throttled_header_shows_backoff_on_narrow_and_wide_terminals() {
        use crate::azure::http::AzError;
        use crate::data::dashboard::{DashboardUpdate, Source};
        use std::time::Duration;

        let (mut app, mono) = demo_app(View::Overview);
        let mut u = DashboardUpdate::default();
        u.errors.push((
            Source::CountMetrics,
            AzError::Throttled {
                retry_after: Some(Duration::from_secs(60)),
            },
        ));
        app.on_data(DataEvent::Dashboard(u), mono, wall());
        for (w, h) in [(80, 24), (100, 30), (160, 45)] {
            let out = text(&render(&app, mono, w, h));
            let header = out.lines().next().unwrap();
            assert!(header.contains("throttled · next in 60s"), "{w}: {header}");
            let out = text(&render(&app, mono + Duration::from_secs(20), w, h));
            assert!(
                out.lines()
                    .next()
                    .unwrap()
                    .contains("throttled · next in 40s")
            );
        }
        let ready = mono + Duration::from_secs(60);
        app.on_tick(ready, wall());
        let out = text(&render(&app, ready, 160, 45));
        let header = out.lines().next().unwrap();
        assert!(!header.contains("throttled · next in"));
        assert!(header.contains("refreshing…"));
    }

    #[test]
    fn old_top_lists_say_how_old_they_are() {
        let (mut app, mono) = demo_app(View::Failures);
        app.store.tops.as_mut().unwrap().at = wall() - chrono::Duration::hours(2);
        let out = text(&render(&app, mono, 160, 45));
        assert!(out.contains("2h old"), "{out}");
    }

    #[test]
    fn failures_top_lists_visible_on_narrow_terminals() {
        let (app, mono) = demo_app(View::Failures);
        for (w, h) in [(80, 24), (100, 30), (119, 40)] {
            let out = text(&render(&app, mono, w, h));
            assert!(out.contains("top failing endpoints"), "{w}x{h}:\n{out}");
            assert!(out.contains("POST /api/orders"), "{w}x{h}");
        }
    }

    fn rect_text(t: &Terminal<TestBackend>, r: Rect) -> String {
        let buf = t.backend().buffer();
        (r.y..r.y + r.height)
            .map(|y| {
                (r.x..r.x + r.width)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn zero_filled_counts_without_telemetry_are_no_data() {
        let (mut app, mono) = demo_app(View::Overview);
        // An app without browser telemetry and without availability tests:
        // the metrics API may still return zero-valued count buckets.
        for id in [
            SeriesId::PageLoadTotal,
            SeriesId::PageLoadNetwork,
            SeriesId::PageLoadProcessing,
            SeriesId::PageLoadSend,
            SeriesId::PageLoadReceive,
            SeriesId::Availability,
        ] {
            app.store.series.remove(&id);
            app.store.totals.remove(&id);
        }
        for id in [SeriesId::ExceptionsBrowser, SeriesId::AvailabilityCount] {
            for p in &mut app.store.series.get_mut(&id).unwrap().value.points {
                p.1 = Some(0.0);
            }
            app.store.totals.get_mut(&id).unwrap().value = 0.0;
        }
        let t = render(&app, mono, 160, 45);
        let browser = layout::overview_layout(Rect::new(0, 0, 160, 45)).panels[7];
        assert!(
            rect_text(&t, browser).contains("no data in range"),
            "{}",
            rect_text(&t, browser)
        );
        app.view = View::Failures;
        let t = render(&app, mono, 160, 45);
        let tests_panel = layout::failures_layout(Rect::new(0, 0, 160, 45)).panels[7];
        assert!(
            rect_text(&t, tests_panel).contains("no data in range"),
            "{}",
            rect_text(&t, tests_panel)
        );
    }

    #[test]
    fn slow_response_tile_shows_seconds() {
        let (mut app, mono) = demo_app(View::Overview);
        app.store
            .totals
            .get_mut(&SeriesId::ServerResponse)
            .unwrap()
            .value = 1500.0;
        let t = render(&app, mono, 160, 45);
        let tile = rect_text(
            &t,
            layout::overview_layout(Rect::new(0, 0, 160, 45)).kpis[5],
        );
        assert!(tile.contains("s avg") && !tile.contains("ms"), "{tile}");
        assert!(
            tile.contains(&crate::ui::widgets::bigdigits::big_text("1.5")[0]),
            "{tile}"
        );
    }

    #[test]
    fn health_response_meters_follow_the_alert_threshold() {
        let (mut app, mono) = demo_app(View::Failures);
        app.store
            .totals
            .get_mut(&SeriesId::ServerResponse)
            .unwrap()
            .value = 1000.0;
        let panel = failures_layout(Rect::new(0, 0, 160, 45)).health;
        let inner = PanelBlock::inner(panel);
        let meter_w = inner.width.saturating_sub(15 + 9 + 1).min(14);
        let off = Theme::new(Depth::TrueColor).color(palette::METER_OFF);
        for (threshold, expected) in [(1000.0, meter_w), (2000.0, meter_w / 2)] {
            app.config.alerts.response_avg_above_ms = threshold;
            let t = render(&app, mono, 160, 45);
            let lit = (0..meter_w)
                .filter(|x| t.backend().buffer()[(inner.x + 15 + x, inner.y + 1)].fg != off)
                .count();
            assert_eq!(lit, expected as usize);
            let memory_row = rect_text(&t, Rect::new(inner.x, inner.y + 4, inner.width, 1));
            assert!(memory_row.contains("memory free"));
            assert!(!memory_row.contains('■'));
        }
    }

    #[test]
    fn non_login_cli_error_does_not_suggest_signing_in() {
        let (mut app, mono) = demo_app(View::Overview);
        app.auth_problem = Some(AuthError::Other("az timed out".into()));
        let out = text(&render(&app, mono, 100, 30));
        assert!(out.contains("Azure CLI error"));
        assert!(out.contains("retrying automatically"));
        assert!(!out.contains("az login"));
    }

    #[test]
    fn long_instance_name_keeps_tabs_visible() {
        let (mut app, mono) = demo_app(View::Overview);
        app.instance.as_mut().unwrap().name = format!("my-{}-prod", "very-long-name".repeat(4));
        for w in [80, 100, 110] {
            let out = text(&render(&app, mono, w, 30));
            let header = out.lines().next().unwrap();
            assert!(header.contains("failures ● 12"), "{w}: {header}");
            assert!(header.contains('…'), "name is shortened: {header}");
        }
    }

    #[test]
    fn long_names_do_not_panic() {
        let (mut app, mono) = demo_app(View::Failures);
        if let Some(t) = app.store.tops.as_mut() {
            t.value.endpoints.insert(
                0,
                TopItem {
                    name: format!("POST /api/{}", "x".repeat(300)),
                    detail: String::new(),
                    n: 99.0,
                },
            );
            t.value.exceptions.insert(
                0,
                TopItem {
                    name: "例外が発生しました😀".repeat(20),
                    detail: "詳細".repeat(50),
                    n: 5.0,
                },
            );
        }
        // 80×24: the side list does not fit and is hidden; it must not panic.
        render(&app, mono, 80, 24);
        let out = text(&render(&app, mono, 160, 45));
        assert!(out.contains('…'), "long names are cut with an ellipsis");
    }

    #[test]
    fn picker_snapshot() {
        let (mut app, mono) = demo_app(View::Overview);
        app.open_picker(true, None);
        app.on_data(DataEvent::Instances(Ok(demo_instances())), mono, wall());
        app.on_key(
            crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Down,
                crossterm::event::KeyModifiers::NONE,
            ),
            mono,
        );
        insta::assert_snapshot!("picker_100x30", render(&app, mono, 100, 30).backend());
        app.picker.as_mut().unwrap().all = None;
        assert!(text(&render(&app, mono, 100, 30)).contains("discovering instances…"));
        app.picker.as_mut().unwrap().all = Some(Err("HTTP 403: denied".into()));
        assert!(text(&render(&app, mono, 100, 30)).contains("HTTP 403: denied"));
    }

    #[test]
    fn picker_without_instance_hides_dashboard() {
        let mut app = App::new(Config::default(), None, TimeRange::H4, 30);
        app.open_picker(false, None);
        let out = text(&render(&app, Instant::now(), 100, 30));
        assert!(out.contains("select application insights"));
        assert!(out.contains("esc quit"));
        assert!(!out.contains("sessions & users"));
        app.toast("ignoring invalid config.toml", Instant::now());
        let out = text(&render(&app, Instant::now(), 100, 30));
        assert!(out.contains("ignoring invalid config.toml"));
        app.on_data(
            DataEvent::Instances(Ok(demo_instances())),
            Instant::now(),
            wall(),
        );
        app.picker.as_mut().unwrap().pending_app = Some("nope".into());
        app.on_data(
            DataEvent::Instances(Ok(demo_instances())),
            Instant::now(),
            wall(),
        );
        let out = text(&render(&app, Instant::now(), 120, 40));
        assert!(out.contains("no instance named nope"));
    }

    #[test]
    fn auth_screen_draws_the_picker_for_recovery() {
        let (mut app, mono) = demo_app(View::Overview);
        app.auth_problem = Some(AuthError::NotLoggedIn("expired".into()));
        app.on_key(crate::app::tests::ch('i'), mono);
        let out = text(&render(&app, mono, 100, 30));
        assert!(out.contains("select application insights"));
        assert!(out.contains("discovering instances"));
    }

    #[test]
    fn failed_top_lists_show_the_error_instead_of_loading() {
        use crate::azure::http::AzError;
        use crate::data::dashboard::{DashboardUpdate, Source};
        let (mut app, mono) = demo_app(View::Failures);
        app.store.tops = None;
        app.on_data(
            DataEvent::Dashboard(DashboardUpdate {
                errors: vec![(
                    Source::Tops,
                    AzError::Http {
                        status: 403,
                        message: "tops denied".into(),
                    },
                )],
                ..Default::default()
            }),
            mono,
            wall(),
        );
        let out = text(&render(&app, mono, 160, 45));
        assert!(!out.contains("loading…"));
        assert!(
            out.lines()
                .skip(1)
                .any(|l| l.contains("HTTP 403: tops denied"))
        );
    }

    #[test]
    fn stale_scalars_and_alert_window_are_visible_while_charts_stay_fresh() {
        use crate::azure::http::AzError;
        use crate::data::dashboard::{DashboardUpdate, Source};
        let (mut app, mono) = demo_app(View::Overview);
        let later = wall() + chrono::Duration::hours(1);
        app.on_data(
            DataEvent::Dashboard(DashboardUpdate {
                series: app
                    .store
                    .series
                    .iter()
                    .map(|(id, v)| (*id, v.value.clone()))
                    .collect(),
                errors: vec![
                    (Source::AlertWindow, AzError::Network("down".into())),
                    (Source::Scalars, AzError::Network("down".into())),
                ],
                ..Default::default()
            }),
            mono,
            later,
        );
        assert!(!app.alerts.failures());
        let theme = Theme::new(Depth::TrueColor);
        let mut t = Terminal::new(TestBackend::new(160, 45)).unwrap();
        t.draw(|f| draw(f, &app, &theme, later.with_timezone(local().offset()), mono))
            .unwrap();
        let layout = overview_layout(Rect::new(0, 0, 160, 45));
        for index in [0, 1, 3, 5] {
            let tile = rect_text(&t, layout.kpis[index]);
            assert!(tile.contains("stale 1h"), "tile {index}: {tile}");
        }
        assert!(!app.store.is_stale(SeriesId::RequestsFailed, later, 30));
        assert!(
            text(&t)
                .lines()
                .next()
                .unwrap()
                .contains("failures · stale 1h")
        );
        app.view = View::Failures;
        t.draw(|f| draw(f, &app, &theme, later.with_timezone(local().offset()), mono))
            .unwrap();
        let panel = rect_text(&t, failures_layout(Rect::new(0, 0, 160, 45)).main);
        assert!(panel.contains("stale 1h"), "{panel}");
        assert!(panel.contains("last known"), "{panel}");
    }

    /// Opens drill-down screens from `panel` by pressing Enter along `path`
    /// (row indexes to select before each further Enter), loading each
    /// screen from the demo backend.
    pub(crate) fn drill_app(view: View, panel: char, path: &[usize]) -> (App, Instant) {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let (mut app, mono) = demo_app(view);
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let key = |c| KeyEvent::new(c, KeyModifiers::NONE);
        app.on_key(key(KeyCode::Char(panel)), mono);
        let mut cmds = app.on_key(key(KeyCode::Enter), mono);
        let mut step = 0;
        loop {
            for c in std::mem::take(&mut cmds) {
                if let crate::app::Command::Fetch(crate::app::DataRequest::Drill {
                    frame_id,
                    instance,
                    range,
                    target,
                }) = c
                {
                    let result = rt.block_on(crate::data::drill::load(
                        &DemoBackend::at(wall()),
                        &instance,
                        range,
                        &target,
                        wall(),
                    ));
                    app.on_data(DataEvent::Drill { frame_id, result }, mono, wall());
                }
            }
            let Some(row) = path.get(step) else { break };
            for _ in 0..*row {
                app.on_key(key(KeyCode::Down), mono);
            }
            cmds = app.on_key(key(KeyCode::Enter), mono);
            step += 1;
        }
        (app, mono)
    }

    #[test]
    fn failing_endpoints_snapshot() {
        let (app, mono) = drill_app(View::Overview, '2', &[]);
        assert!(matches!(app.drill[0].data, crate::app::FrameData::Ready(_)));
        let t = render(&app, mono, 100, 30);
        let out = text(&t);
        assert!(out.contains("overview › failed requests"), "{out}");
        assert!(out.contains("POST /api/orders"));
        insta::assert_snapshot!("failing_endpoints_100x30", t.backend());
    }

    #[test]
    fn endpoint_screen_snapshot() {
        let (app, mono) = drill_app(View::Overview, '2', &[0]);
        assert!(matches!(
            app.drill[1].data,
            crate::app::FrameData::Ready(crate::data::drill::ScreenData::Endpoint(_))
        ));
        let t = render(&app, mono, 120, 34);
        let out = text(&t);
        assert!(out.contains("overview › failed requests › POST /api/orders"));
        assert!(out.contains("SqlException"));
        insta::assert_snapshot!("endpoint_screen_120x34", t.backend());
    }

    #[test]
    fn request_screen_snapshot() {
        let (app, mono) = drill_app(View::Overview, '2', &[0, 0]);
        let t = render(&app, mono, 120, 34);
        let out = text(&t);
        assert!(out.contains("› request"));
        assert!(out.contains("SqlException"));
        assert!(out.contains("end-to-end transaction"));
        insta::assert_snapshot!("request_screen_120x34", t.backend());
    }

    #[test]
    fn slow_endpoints_snapshot() {
        let (app, mono) = drill_app(View::Overview, '3', &[]);
        let t = render(&app, mono, 100, 30);
        assert!(text(&t).contains("overview › slowest endpoints"));
        insta::assert_snapshot!("slow_endpoints_100x30", t.backend());
    }

    #[test]
    fn every_drillable_panel_renders() {
        for (view, panel) in [
            (View::Overview, '5'),
            (View::Overview, '4'),
            (View::Overview, '6'),
            (View::Overview, '7'),
            (View::Overview, '8'),
            (View::Overview, '9'),
            (View::Failures, '9'),
        ] {
            let (app, mono) = drill_app(view, panel, &[0]);
            let out = text(&render(&app, mono, 100, 30));
            assert!(!out.contains("not implemented"), "{view:?} {panel}: {out}");
        }
    }

    #[test]
    fn long_transactions_scroll_to_the_selection() {
        let (mut app, mono) = drill_app(View::Overview, '2', &[0, 0]);
        let f = app.drill.last_mut().unwrap();
        f.data = crate::app::FrameData::Ready(crate::data::drill::ScreenData::Request(
            crate::app::tests::long_transaction(60, 50),
        ));
        f.selected[0] = 50;
        let out = text(&render(&app, mono, 120, 34));
        assert!(out.contains("call 50"), "{out}");
        assert!(out.contains("51 of 60"), "{out}");
    }

    #[test]
    fn drill_loading_and_failed_states() {
        let (mut app, mono) = drill_app(View::Overview, '2', &[]);
        app.drill[0].data = crate::app::FrameData::Loading;
        assert!(text(&render(&app, mono, 100, 30)).contains("loading…"));
        app.drill[0].data = crate::app::FrameData::Failed("HTTP 500: boom".into());
        let out = text(&render(&app, mono, 100, 30));
        assert!(out.contains("HTTP 500: boom") && out.contains("r to retry"));
    }

    #[test]
    fn long_endpoint_names_truncate() {
        let (mut app, mono) = drill_app(View::Overview, '2', &[]);
        if let crate::app::FrameData::Ready(crate::data::drill::ScreenData::List(l)) =
            &mut app.drill[0].data
        {
            l.rows[0].cells[0] =
                crate::data::drill::Cell::Text(format!("GET /{}", "y".repeat(300)));
        }
        let out = text(&render(&app, mono, 80, 24));
        assert!(out.contains('…'));
    }

    #[test]
    fn all_sizes_render() {
        for view in [View::Overview, View::Failures] {
            let (app, mono) = demo_app(view);
            for w in (20..=200).step_by(7) {
                for h in (5..=60).step_by(5) {
                    render(&app, mono, w, h);
                }
            }
        }
    }

    #[test]
    fn drill_and_picker_states_render_at_all_sizes() {
        let mut cases = Vec::new();
        for (view, panel) in [
            (View::Overview, '2'),
            (View::Overview, '3'),
            (View::Overview, '4'),
            (View::Overview, '5'),
            (View::Overview, '6'),
            (View::Overview, '7'),
            (View::Overview, '8'),
            (View::Overview, '9'),
            (View::Failures, '9'),
        ] {
            for path in [&[][..], &[0][..]] {
                cases.push(drill_app(view, panel, path));
            }
        }
        cases.push(drill_app(View::Overview, '2', &[0, 0]));
        let (mut loading, mono) = drill_app(View::Overview, '2', &[]);
        loading.drill[0].data = crate::app::FrameData::Loading;
        cases.push((loading, mono));
        let (mut failed, mono) = drill_app(View::Overview, '2', &[]);
        failed.drill[0].data = crate::app::FrameData::Failed("failed to load".into());
        cases.push((failed, mono));
        for all in [
            None,
            Some(Err("discovery failed".into())),
            Some(Ok(vec![])),
            Some(Ok(demo_instances())),
        ] {
            let mut app = App::new(Config::default(), None, TimeRange::H4, 30);
            app.open_picker(false, None);
            app.picker.as_mut().unwrap().all = all;
            cases.push((app, Instant::now()));
        }
        for (app, mono) in cases {
            for w in (20..=200).step_by(3) {
                for h in (5..=60).step_by(3) {
                    render(&app, mono, w, h);
                }
            }
        }
    }
}
