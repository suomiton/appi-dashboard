//! Drill-down screens: breadcrumb, list screens, and loading/error states.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::app::{DrillFrame, FrameData, View};
use crate::data::drill::{
    Cell, DrillTarget, EndpointData, ListData, NumFmt, RequestData, ScreenData, Severity, TxNode,
};
use crate::data::format::{fmt_bytes, fmt_ms, fmt_num, truncate_width};
use crate::ui::common::{Ctx, put_centered, put_line};
use crate::ui::theme::{Rgb, palette};
use crate::ui::widgets::braille::sparkline;
use crate::ui::widgets::panel::{Emphasis, PanelBlock};

pub fn render(buf: &mut Buffer, area: Rect, ctx: &Ctx) {
    if area.height < 3 {
        return;
    }
    breadcrumb(buf, Rect::new(area.x, area.y, area.width, 1), ctx);
    let body = Rect::new(area.x, area.y + 1, area.width, area.height - 2);
    let footer = Rect::new(area.x, area.y + area.height - 1, area.width, 1);
    if let Some(frame) = ctx.app.drill.last() {
        match &frame.data {
            FrameData::Loading => state_panel(buf, body, ctx, frame, "loading…", palette::DIM),
            FrameData::Failed(msg) => state_panel(
                buf,
                body,
                ctx,
                frame,
                &format!("{msg} · r to retry"),
                palette::RED,
            ),
            FrameData::Ready(ScreenData::List(l)) => list_screen(buf, body, ctx, frame, l),
            FrameData::Ready(ScreenData::Endpoint(e)) => endpoint_screen(buf, body, ctx, frame, e),
            FrameData::Ready(ScreenData::Request(r)) => request_screen(buf, body, ctx, r),
        }
    }
    footer_keys(buf, footer, ctx);
}

fn breadcrumb(buf: &mut Buffer, area: Rect, ctx: &Ctx) {
    let view = match ctx.app.view {
        View::Overview => "overview",
        View::Failures => "failures",
    };
    let mut spans = vec![ctx.s(format!(" {view}"), palette::BLUE)];
    for f in &ctx.app.drill {
        spans.push(ctx.s(" › ", palette::DIM));
        spans.push(ctx.s(f.target.crumb(), palette::BLUE));
    }
    put_line(buf, area, 0, &Line::from(spans));
}

pub(crate) fn footer_keys(buf: &mut Buffer, area: Rect, ctx: &Ctx) {
    if let Some((msg, _)) = &ctx.app.toast {
        put_line(
            buf,
            area,
            0,
            &Line::from(ctx.s(format!(" {msg}"), palette::YELLOW)),
        );
        return;
    }
    let on_request = ctx
        .app
        .drill
        .last()
        .is_some_and(|f| matches!(f.target, DrillTarget::RequestDetail { .. }));
    let request_keys: &[(&str, &str)] = &[
        ("c", "copy operation_Id"),
        ("o", "open in Azure portal"),
        ("r", "reload"),
        ("esc", "back"),
        ("tab", "switch view"),
    ];
    let list_keys: &[(&str, &str)] = &[
        ("↑↓", "select"),
        ("↵", "open"),
        ("s", "sort"),
        ("r", "reload"),
        ("o", "portal"),
        ("esc", "back"),
        ("tab", "switch view"),
    ];
    let keys = if on_request { request_keys } else { list_keys };
    let mut spans = vec![ctx.s(" ", palette::DIM)];
    for (k, label) in keys {
        spans.push(ctx.s(*k, palette::ORANGE));
        spans.push(ctx.s(format!(" {label}  "), palette::DIM));
    }
    put_line(buf, area, 0, &Line::from(spans));
}

fn state_panel(buf: &mut Buffer, area: Rect, ctx: &Ctx, frame: &DrillFrame, msg: &str, color: Rgb) {
    PanelBlock {
        title: &frame.target.crumb(),
        hotkey: None,
        right: None,
        emphasis: Emphasis::Focused,
        stale: None,
        theme: ctx.theme,
    }
    .render(area, buf);
    put_centered(buf, PanelBlock::inner(area), msg, ctx.theme.fg(color));
}

fn severity_color(s: Severity) -> Rgb {
    match s {
        Severity::Bad => palette::RED,
        Severity::Warn => palette::YELLOW,
        Severity::Normal => palette::CYAN,
    }
}

/// Column widths: text columns share what fixed-width columns leave.
fn widths(l: &ListData, total: u16) -> Vec<u16> {
    let fixed = |c: Option<&Cell>| -> Option<u16> {
        match c? {
            Cell::Num(..) => Some(9),
            Cell::Time(_) => Some(10),
            Cell::Codes(_) => Some(20),
            Cell::Trend(_) => Some(12),
            Cell::Text(_) => None,
        }
    };
    let kinds: Vec<Option<u16>> = (0..l.columns.len())
        .map(|i| fixed(l.rows.first().and_then(|r| r.cells.get(i))))
        .collect();
    let used: u16 = kinds.iter().flatten().sum::<u16>() + 2;
    let flex = kinds.iter().filter(|k| k.is_none()).count().max(1) as u16;
    let share = (total.saturating_sub(used) / flex).max(8);
    kinds.into_iter().map(|k| k.unwrap_or(share)).collect()
}

fn cell_spans(
    ctx: &Ctx,
    cell: &Cell,
    width: u16,
    severity: Severity,
    first: bool,
) -> Vec<Span<'static>> {
    let w = width.saturating_sub(1) as usize;
    let pad = |s: String| format!("{s:<w$} ");
    match cell {
        Cell::Text(t) => vec![ctx.s(
            pad(truncate_width(t, w)),
            if first { palette::TEXT } else { palette::SOFT },
        )],
        Cell::Num(v, f) => {
            let s = match f {
                NumFmt::Count => fmt_num(*v),
                NumFmt::Ms => fmt_ms(*v),
                NumFmt::Pct => format!("{}%", fmt_num(*v)),
                NumFmt::Bytes => fmt_bytes(*v),
            };
            let color = match severity {
                Severity::Normal => palette::TEXT,
                other => severity_color(other),
            };
            vec![ctx.s(format!("{:>w$} ", truncate_width(&s, w)), color)]
        }
        Cell::Time(t) => vec![ctx.s(
            pad(t.with_timezone(&ctx.offset).format("%H:%M:%S").to_string()),
            palette::SOFT,
        )],
        Cell::Codes(codes) => {
            let mut spans = Vec::new();
            let mut used = 0;
            for (code, n) in codes {
                let text = format!("{code}×{} ", fmt_num(*n));
                if used + text.chars().count() > w {
                    break;
                }
                used += text.chars().count();
                let color = if code.starts_with('5') {
                    palette::RED
                } else if code.starts_with('4') {
                    palette::YELLOW
                } else {
                    palette::SOFT
                };
                spans.push(ctx.s(text, color));
            }
            spans.push(ctx.s(" ".repeat(w + 1 - used), palette::SOFT));
            spans
        }
        Cell::Trend(v) => {
            let w2 = w.saturating_sub(1);
            vec![ctx.s(format!("{} ", sparkline(v, w2)), severity_color(severity))]
        }
    }
}

fn list_screen(buf: &mut Buffer, area: Rect, ctx: &Ctx, frame: &DrillFrame, l: &ListData) {
    PanelBlock {
        title: &l.title,
        hotkey: None,
        right: Some(Line::from(ctx.s(l.right.clone(), palette::DIM))),
        emphasis: Emphasis::Focused,
        stale: None,
        theme: ctx.theme,
    }
    .render(area, buf);
    let inner = PanelBlock::inner(area).intersection(buf.area);
    if inner.height < 2 || inner.width < 10 {
        return;
    }
    let table = Rect::new(inner.x + 2, inner.y, inner.width - 2, inner.height);
    let ws = widths(l, table.width);
    let mut header = Vec::new();
    for (i, (name, w)) in l.columns.iter().zip(&ws).enumerate() {
        let arrow = match frame.sort {
            Some((c, true)) if c == i => "▼",
            Some((c, false)) if c == i => "▲",
            _ => "",
        };
        let label = format!("{name}{arrow}");
        let w = *w as usize;
        header.push(ctx.s(
            format!("{:<w$}", truncate_width(&label, w.saturating_sub(1))),
            palette::DIM,
        ));
    }
    put_line(buf, table, 0, &Line::from(header));
    if l.rows.is_empty() {
        put_centered(
            buf,
            Rect::new(inner.x, inner.y + 1, inner.width, inner.height - 1),
            "nothing in this time range",
            ctx.theme.fg(palette::GREEN),
        );
        return;
    }
    let visible = (inner.height - 1) as usize;
    let selected = frame.selected[0].min(l.rows.len() - 1);
    let first = selected.saturating_sub(visible.saturating_sub(1));
    for (i, row) in l.rows.iter().enumerate().skip(first).take(visible) {
        let y = 1 + (i - first) as u16;
        let mut spans = Vec::new();
        for (c, (cell, w)) in row.cells.iter().zip(&ws).enumerate() {
            spans.extend(cell_spans(ctx, cell, *w, row.severity, c == 0));
        }
        put_line(buf, table, y, &Line::from(spans));
        if i == selected {
            buf.set_string(inner.x, inner.y + y, "▶", ctx.theme.fg(palette::BLUE));
            buf.set_style(
                Rect::new(inner.x, inner.y + y, inner.width, 1),
                Style::new().bg(ctx.theme.color(palette::SEL_BG)),
            );
        }
    }
}

fn sub_panel(buf: &mut Buffer, area: Rect, ctx: &Ctx, title: &str, focused: bool) -> Rect {
    PanelBlock {
        title,
        hotkey: None,
        right: None,
        emphasis: if focused {
            Emphasis::Focused
        } else {
            Emphasis::Normal
        },
        stale: None,
        theme: ctx.theme,
    }
    .render(area, buf);
    PanelBlock::inner(area).intersection(buf.area)
}

fn bar(n: f64, max: f64, width: usize) -> String {
    if max <= 0.0 || width == 0 {
        return String::new();
    }
    let cells = ((n / max) * width as f64).round().clamp(1.0, width as f64) as usize;
    format!("{:<width$}", "█".repeat(cells))
}

fn mark_selected(buf: &mut Buffer, inner: Rect, y: u16, ctx: &Ctx) {
    buf.set_string(inner.x, inner.y + y, "▶", ctx.theme.fg(palette::BLUE));
    buf.set_style(
        Rect::new(inner.x, inner.y + y, inner.width, 1),
        Style::new().bg(ctx.theme.color(palette::SEL_BG)),
    );
}

fn code_color(code: &str) -> Rgb {
    if code.starts_with('5') {
        palette::RED
    } else if code.starts_with('4') {
        palette::YELLOW
    } else {
        palette::GREEN
    }
}

fn endpoint_screen(buf: &mut Buffer, area: Rect, ctx: &Ctx, frame: &DrillFrame, e: &EndpointData) {
    let top_h = ((area.height as u32 * 40 / 100) as u16)
        .max(5)
        .min(area.height);
    let half = area.width / 2;
    let exc_area = Rect::new(area.x, area.y, half, top_h);
    let dep_area = Rect::new(area.x + half, area.y, area.width - half, top_h);
    let recent_area = Rect::new(area.x, area.y + top_h, area.width, area.height - top_h);
    let sel = |sub: usize| frame.selected[sub];

    let inner = sub_panel(buf, exc_area, ctx, "exception types", frame.sub == 0);
    let max = e.exceptions.iter().map(|x| x.n).fold(0.0, f64::max);
    let w = inner.width as usize;
    if e.exceptions.is_empty() {
        put_line(buf, inner, 0, &Line::from(ctx.s("  none", palette::GREEN)));
    }
    for (i, x) in e.exceptions.iter().enumerate().take(inner.height as usize) {
        let name_w = (w / 2).saturating_sub(2);
        let bar_w = w.saturating_sub(name_w + 10).min(14);
        let line = Line::from(vec![
            ctx.s("  ", palette::DIM),
            ctx.s(
                format!("{:<name_w$} ", truncate_width(&x.name, name_w)),
                palette::PURPLE,
            ),
            ctx.s(bar(x.n, max, bar_w), palette::PURPLE),
            ctx.s(format!(" {:>4}", fmt_num(x.n)), palette::TEXT),
        ]);
        put_line(buf, inner, i as u16, &line);
        if frame.sub == 0 && i == sel(0) {
            mark_selected(buf, inner, i as u16, ctx);
        }
    }

    let inner = sub_panel(buf, dep_area, ctx, "failing dependencies", frame.sub == 1);
    let max = e.dependencies.iter().map(|x| x.n).fold(0.0, f64::max);
    let w = inner.width as usize;
    if e.dependencies.is_empty() {
        put_line(buf, inner, 0, &Line::from(ctx.s("  none", palette::GREEN)));
    }
    for (i, d) in e
        .dependencies
        .iter()
        .enumerate()
        .take(inner.height as usize)
    {
        let name_w = (w / 2).saturating_sub(2);
        let bar_w = w.saturating_sub(name_w + 10).min(14);
        let label = truncate_width(&format!("{} {}", d.dep_type, d.target), name_w);
        let line = Line::from(vec![
            ctx.s("  ", palette::DIM),
            ctx.s(format!("{label:<name_w$} "), palette::SOFT),
            ctx.s(bar(d.n, max, bar_w), palette::ORANGE),
            ctx.s(format!(" {:>4}", fmt_num(d.n)), palette::TEXT),
        ]);
        put_line(buf, inner, i as u16, &line);
        if frame.sub == 1 && i == sel(1) {
            mark_selected(buf, inner, i as u16, ctx);
        }
    }

    let inner = sub_panel(buf, recent_area, ctx, "recent failures", frame.sub == 2);
    if inner.height == 0 {
        return;
    }
    put_line(
        buf,
        inner,
        0,
        &Line::from(ctx.s(
            "  time      code  duration  instance          exception",
            palette::DIM,
        )),
    );
    if e.recent.is_empty() {
        put_line(buf, inner, 1, &Line::from(ctx.s("  none", palette::GREEN)));
    }
    let visible = inner.height.saturating_sub(1) as usize;
    let first = sel(2).saturating_sub(visible.saturating_sub(1));
    for (i, r) in e.recent.iter().enumerate().skip(first).take(visible) {
        let y = 1 + (i - first) as u16;
        let line = Line::from(vec![
            ctx.s(
                format!("  {}  ", r.at.with_timezone(&ctx.offset).format("%H:%M:%S")),
                palette::SOFT,
            ),
            ctx.s(format!("{:<4}  ", r.code), code_color(&r.code)),
            ctx.s(format!("{:>8}  ", fmt_ms(r.duration_ms)), palette::TEXT),
            ctx.s(
                format!("{:<16}  ", truncate_width(&r.instance, 16)),
                palette::SOFT,
            ),
            ctx.s(
                r.exception_type.clone().unwrap_or_default(),
                palette::PURPLE,
            ),
        ]);
        put_line(buf, inner, y, &line);
        if frame.sub == 2 && i == sel(2) {
            mark_selected(buf, inner, y, ctx);
        }
    }
}

/// Splits `text` into at most `max_lines` lines of `width` columns.
fn wrap(text: &str, width: usize, max_lines: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let candidate = if current.is_empty() {
            word.to_string()
        } else {
            format!("{current} {word}")
        };
        if unicode_width::UnicodeWidthStr::width(candidate.as_str()) > width && !current.is_empty()
        {
            lines.push(std::mem::take(&mut current));
            current = word.to_string();
        } else {
            current = candidate;
        }
        if lines.len() == max_lines {
            break;
        }
    }
    if !current.is_empty() && lines.len() < max_lines {
        lines.push(current);
    }
    lines
        .into_iter()
        .map(|l| truncate_width(&l, width))
        .collect()
}

/// Whether each node is the last child at its depth (for └ vs ├).
fn last_flags(nodes: &[TxNode]) -> Vec<bool> {
    (0..nodes.len())
        .map(|i| {
            let d = nodes[i].depth;
            nodes[i + 1..]
                .iter()
                .find(|n| n.depth <= d)
                .is_none_or(|n| n.depth < d)
        })
        .collect()
}

fn request_screen(buf: &mut Buffer, area: Rect, ctx: &Ctx, r: &RequestData) {
    let frames = r.exception.as_ref().map_or(0, |e| e.frames.len()) as u16;
    let wanted = 2 + 2 + if r.exception.is_some() { 5 + frames } else { 0 };
    let top_h = wanted.min(area.height * 60 / 100).max(5).min(area.height);
    let top = Rect::new(area.x, area.y, area.width, top_h);
    let bottom = Rect::new(area.x, area.y + top_h, area.width, area.height - top_h);

    let inner = sub_panel(buf, top, ctx, "request", false);
    let w = inner.width.saturating_sub(2) as usize;
    let mut lines: Vec<Line> = Vec::new();
    match &r.request {
        None => lines.push(Line::from(
            ctx.s(" request not found in this time range", palette::DIM),
        )),
        Some(q) => {
            lines.push(Line::from(vec![
                ctx.s(" result ", palette::DIM),
                ctx.s(q.code.clone(), code_color(&q.code)),
                ctx.s("   duration ", palette::DIM),
                ctx.s(fmt_ms(q.duration_ms), palette::TEXT),
                ctx.s("   instance ", palette::DIM),
                ctx.s(q.instance.clone(), palette::TEXT),
                ctx.s("   at ", palette::DIM),
                ctx.s(
                    q.at.with_timezone(&ctx.offset)
                        .format("%H:%M:%S")
                        .to_string(),
                    palette::TEXT,
                ),
            ]));
            lines.push(Line::from(vec![
                ctx.s(" operation_Id ", palette::DIM),
                ctx.s(q.operation_id.clone(), palette::TEXT),
                ctx.s("   session ", palette::DIM),
                ctx.s(truncate_width(&q.session_id, 12), palette::TEXT),
                ctx.s("   user ", palette::DIM),
                ctx.s(truncate_width(&q.user_id, 12), palette::TEXT),
            ]));
        }
    }
    if let Some(e) = &r.exception {
        lines.push(Line::default());
        lines.push(Line::from(
            ctx.s(format!(" {}", e.exception_type), palette::PURPLE),
        ));
        for l in wrap(&e.message, w.saturating_sub(2), 3) {
            lines.push(Line::from(ctx.s(format!("   {l}"), palette::TEXT)));
        }
        for f in &e.frames {
            let rest = f.strip_prefix("at ").unwrap_or(f);
            lines.push(Line::from(vec![
                ctx.s("   at ", palette::DIM),
                ctx.s(truncate_width(rest, w.saturating_sub(5)), palette::SOFT),
            ]));
        }
    }
    for (i, l) in lines.iter().enumerate().take(inner.height as usize) {
        put_line(buf, inner, i as u16, l);
    }

    let inner = sub_panel(buf, bottom, ctx, "end-to-end transaction", false);
    if r.transaction.is_empty() {
        put_line(
            buf,
            inner,
            0,
            &Line::from(ctx.s(" no telemetry for this operation", palette::DIM)),
        );
        return;
    }
    let span = r
        .transaction
        .iter()
        .map(|n| n.offset_ms + n.duration_ms)
        .fold(1.0, f64::max);
    let bar_w = 24usize;
    let label_w = (inner.width as usize).saturating_sub(bar_w + 14).max(10);
    let last = last_flags(&r.transaction);
    for (i, n) in r.transaction.iter().enumerate().take(inner.height as usize) {
        let prefix = if n.depth == 0 {
            String::new()
        } else {
            format!(
                "{}{}",
                "  ".repeat(n.depth - 1),
                if last[i] { "└ " } else { "├ " }
            )
        };
        let label = truncate_width(&format!("{prefix}{}", n.label), label_w);
        let start = ((n.offset_ms / span) * bar_w as f64).floor() as usize;
        let len = ((n.duration_ms / span) * bar_w as f64).round() as usize;
        let start = start.min(bar_w.saturating_sub(1));
        let bar = if len == 0 {
            "▏".to_string()
        } else {
            "█".repeat(len.min(bar_w - start))
        };
        let failed = n.success == Some(false);
        let line = Line::from(vec![
            ctx.s(
                format!(" {label:<label_w$} "),
                if n.depth == 0 {
                    palette::TEXT
                } else {
                    palette::SOFT
                },
            ),
            ctx.s(" ".repeat(start), palette::DIM),
            ctx.s(
                format!("{bar:<width$}", width = bar_w - start),
                if failed { palette::RED } else { palette::GREEN },
            ),
            ctx.s(format!(" {:>7} ", fmt_ms(n.duration_ms)), palette::TEXT),
            match n.success {
                Some(false) => ctx.s("✕", palette::RED),
                Some(true) => ctx.s("✓", palette::GREEN),
                None => ctx.s("·", palette::DIM),
            },
        ]);
        put_line(buf, inner, i as u16, &line);
    }
}
