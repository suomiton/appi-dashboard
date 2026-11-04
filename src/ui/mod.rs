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
use crate::data::format::fmt_age;
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
                footer(buf, l.footer, &ctx, mono);
            }
            View::Failures => {
                let l = failures_layout(area);
                header(buf, l.header, &ctx, now, mono);
                failures::render(buf, &l, &ctx);
                footer(buf, l.footer, &ctx, mono);
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
    let sep = || ctx.s(" │ ", palette::DIM);
    let mut left: Vec<Span> = Vec::new();
    let name = app
        .instance
        .as_ref()
        .map(|i| i.name.clone())
        .unwrap_or_else(|| "no instance".into());
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
    let label = if alerting {
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
    } else if let Some(s) = app.seconds_to_refresh(mono) {
        left.push(ctx.s(format!("next {s}s ⟳"), palette::DIM));
    }
    if let Some(s) = app.store.scalars.as_ref().and_then(|s| s.value.servers_10m) {
        left.push(sep());
        left.push(ctx.s(format!("● {s:.0} servers"), palette::GREEN));
    }

    let mut right: Vec<Span> = Vec::new();
    if let (Some((_, err)), Some(ok)) = (&app.store.last_error, app.store.last_success) {
        let age = ctx.now - ok;
        if age > chrono::Duration::seconds(2 * app.refresh_secs as i64) {
            right.push(ctx.s(format!("⚠ data {} old  ", fmt_age(age)), palette::YELLOW));
            let _ = err;
        }
    }
    right.push(ctx.s(now.format("%H:%M:%S").to_string(), palette::CYAN));
    if area.width >= 120 {
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

fn footer(buf: &mut Buffer, area: Rect, ctx: &Ctx, mono: Instant) {
    if let Some((msg, _)) = &ctx.app.toast {
        let _ = mono;
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
        title: "Azure CLI sign-in needed",
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
        AuthError::NotLoggedIn(msg) | AuthError::Other(msg) => (
            msg.clone(),
            "Run \"az login\" in another terminal. Retrying automatically.".to_string(),
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
            View::Failures,
            wall(),
        ));
        app.on_tick(mono);
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
}
