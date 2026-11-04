//! Responsive screen layouts.

use ratatui::layout::{Constraint, Layout, Rect};

pub const MIN_WIDTH: u16 = 80;
pub const MIN_HEIGHT: u16 = 24;

pub struct OverviewLayout {
    pub header: Rect,
    /// Seven big-number tiles; empty in the compact layout.
    pub kpis: Vec<Rect>,
    /// One-line headline strip used instead of tiles when space is short.
    pub kpi_strip: Option<Rect>,
    /// Nine chart panels in hotkey order.
    pub panels: Vec<Rect>,
    pub footer: Rect,
}

pub struct FailuresLayout {
    pub header: Rect,
    pub main: Rect,
    pub health: Rect,
    /// Eight smaller panels in hotkey order (3–9, 0).
    pub panels: Vec<Rect>,
    pub footer: Rect,
}

fn frame(area: Rect) -> (Rect, Rect, Rect) {
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(area);
    (header, body, footer)
}

/// Splits `area` into `n` cells, `cols` per row, rows filled top to bottom.
pub fn grid(area: Rect, n: usize, cols: usize) -> Vec<Rect> {
    if n == 0 || cols == 0 {
        return Vec::new();
    }
    let rows = n.div_ceil(cols);
    let row_rects = Layout::vertical(vec![Constraint::Ratio(1, rows as u32); rows]).split(area);
    let mut out = Vec::with_capacity(n);
    for (r, row) in row_rects.iter().enumerate() {
        let in_row = cols.min(n - r * cols);
        let cells =
            Layout::horizontal(vec![Constraint::Ratio(1, in_row as u32); in_row]).split(*row);
        out.extend(cells.iter().copied());
    }
    out
}

pub fn overview_layout(area: Rect) -> OverviewLayout {
    let (header, body, footer) = frame(area);
    let wide = area.width >= 120 && area.height >= 34;
    let cols = if area.width >= 120 { 3 } else { 2 };
    if wide {
        let [kpi_row, grid_area] =
            Layout::vertical([Constraint::Length(6), Constraint::Fill(1)]).areas(body);
        let kpis = Layout::horizontal(vec![Constraint::Ratio(1, 7); 7])
            .split(kpi_row)
            .to_vec();
        OverviewLayout {
            header,
            kpis,
            kpi_strip: None,
            panels: grid(grid_area, 9, cols),
            footer,
        }
    } else {
        let [strip, grid_area] =
            Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(body);
        OverviewLayout {
            header,
            kpis: Vec::new(),
            kpi_strip: Some(strip),
            panels: grid(grid_area, 9, cols),
            footer,
        }
    }
}

pub fn failures_layout(area: Rect) -> FailuresLayout {
    let (header, body, footer) = frame(area);
    let top_h = ((body.height as u32 * 45 / 100) as u16)
        .max(8)
        .min(body.height);
    let [top, rest] =
        Layout::vertical([Constraint::Length(top_h), Constraint::Fill(1)]).areas(body);
    let main_pct = if area.width >= 120 { 70 } else { 64 };
    let [main, health] = Layout::horizontal([
        Constraint::Percentage(main_pct),
        Constraint::Percentage(100 - main_pct),
    ])
    .areas(top);
    let cols = if area.width >= 100 { 4 } else { 2 };
    let panels = if cols == 4 {
        let [a, b] =
            Layout::vertical([Constraint::Percentage(60), Constraint::Percentage(40)]).areas(rest);
        let mut v = grid(a, 4, 4);
        v.extend(grid(b, 4, 4));
        v
    } else {
        grid(rest, 8, 2)
    };
    FailuresLayout {
        header,
        main,
        health,
        panels,
        footer,
    }
}

/// A rectangle of at most `w`×`h` centered in `area`.
pub fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    )
}
