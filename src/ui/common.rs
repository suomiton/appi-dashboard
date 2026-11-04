//! Pieces shared by the dashboard screens.

use chrono::{DateTime, Utc};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::app::{App, PanelId, hotkey};
use crate::data::alerts::AlertKind;
use crate::data::catalog::SeriesId;
use crate::data::format::{fmt_age, fmt_num, truncate_width};
use crate::ui::theme::{Gradient, Rgb, Theme, palette};
use crate::ui::widgets::braille::AreaChart;
use crate::ui::widgets::panel::{Emphasis, PanelBlock};

/// Everything a screen needs to draw.
pub struct Ctx<'a> {
    pub app: &'a App,
    pub theme: &'a Theme,
    pub now: DateTime<Utc>,
}

impl Ctx<'_> {
    pub fn s(&self, text: impl Into<String>, color: Rgb) -> Span<'static> {
        Span::styled(text.into(), self.theme.fg(color))
    }

    pub fn total(&self, id: SeriesId) -> Option<f64> {
        self.app.store.total(id)
    }

    /// A total formatted with `f`, or "—" when there is no data.
    pub fn fmt_total(&self, id: SeriesId, f: impl Fn(f64) -> String) -> String {
        self.total(id).map(f).unwrap_or_else(|| "—".into())
    }

    pub fn num(&self, v: Option<f64>) -> String {
        v.map(fmt_num).unwrap_or_else(|| "—".into())
    }

    pub fn values(&self, id: SeriesId) -> Vec<Option<f64>> {
        self.app
            .store
            .series
            .get(&id)
            .map(|s| s.value.values())
            .unwrap_or_default()
    }

    pub fn stale(&self, id: SeriesId) -> Option<String> {
        let store = &self.app.store;
        store
            .is_stale(id, self.now, self.app.refresh_secs)
            .then(|| store.age(id, self.now).map(fmt_age))
            .flatten()
    }

    pub fn emphasis(&self, panel: PanelId) -> Emphasis {
        let alert = match panel {
            PanelId::FailedRequests | PanelId::FailuresMain => {
                self.app.alerts.is(AlertKind::Failures)
            }
            PanelId::ExceptionsDeps | PanelId::DependencyFailures => {
                self.app.alerts.is(AlertKind::ExceptionsDeps)
            }
            PanelId::Availability => self.app.alerts.is(AlertKind::Availability),
            PanelId::ServerResponse => self.app.alerts.is(AlertKind::Response),
            _ => false,
        };
        let focused = self.app.focus == Some(panel);
        match (alert, focused) {
            (true, true) => Emphasis::AlertFocused,
            (true, false) => Emphasis::Alert,
            (false, true) => Emphasis::Focused,
            (false, false) => Emphasis::Normal,
        }
    }
}

/// Writes `line` into one row of `area`, cut to its width.
pub fn put_line(buf: &mut Buffer, area: Rect, row: u16, line: &Line) {
    let area = area.intersection(buf.area);
    if row < area.height {
        buf.set_line(area.x, area.y + row, line, area.width);
    }
}

/// Writes `text` centered in the middle row of `area`.
pub fn put_centered(buf: &mut Buffer, area: Rect, text: &str, style: Style) {
    let area = area.intersection(buf.area);
    if area.width == 0 || area.height == 0 {
        return;
    }
    let t = truncate_width(text, area.width as usize);
    let w = unicode_width::UnicodeWidthStr::width(t.as_str()) as u16;
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    buf.set_string(x, area.y + area.height / 2, t, style);
}

pub struct ChartSpec<'a> {
    pub panel: PanelId,
    pub title: &'a str,
    pub right: Option<Line<'static>>,
    pub top: (SeriesId, Gradient),
    pub bottom: Option<(SeriesId, Gradient)>,
    pub max: Option<f64>,
    pub legend: Line<'static>,
}

/// A bordered panel with an area chart (optionally a mirrored pair) and a
/// legend line, or "no data in range" when its series are empty.
pub fn chart_panel(buf: &mut Buffer, area: Rect, ctx: &Ctx, spec: ChartSpec) {
    PanelBlock {
        title: spec.title,
        hotkey: hotkey(ctx.app.view, spec.panel),
        right: spec.right,
        emphasis: ctx.emphasis(spec.panel),
        stale: ctx.stale(spec.top.0),
        theme: ctx.theme,
    }
    .render(area, buf);
    let inner = PanelBlock::inner(area);
    chart_body(
        buf,
        inner,
        ctx,
        spec.top,
        spec.bottom,
        spec.max,
        &spec.legend,
    );
}

/// A chart (optionally a mirrored pair) with a legend on its last row, or
/// "no data in range" when its series are empty.
pub fn chart_body(
    buf: &mut Buffer,
    inner: Rect,
    ctx: &Ctx,
    top: (SeriesId, Gradient),
    bottom: Option<(SeriesId, Gradient)>,
    max: Option<f64>,
    legend: &Line,
) {
    let inner = inner.intersection(buf.area);
    if inner.height == 0 || inner.width == 0 {
        return;
    }
    let chart_h = if inner.height >= 2 {
        inner.height - 1
    } else {
        inner.height
    };
    let chart = Rect::new(inner.x, inner.y, inner.width, chart_h);
    if inner.height >= 2 {
        put_line(buf, inner, inner.height - 1, legend);
    }
    let has = |id: SeriesId| ctx.app.store.has_data(id);
    let any = has(top.0) || bottom.is_some_and(|(b, _)| has(b));
    if !any {
        put_centered(buf, chart, "no data in range", ctx.theme.fg(palette::DIM));
        return;
    }
    let top_values = ctx.values(top.0);
    match bottom {
        Some((bottom, bottom_grad)) if chart.height >= 2 => {
            let top_h =
                ((chart.height as u32 * 60).div_ceil(100) as u16).clamp(1, chart.height - 1);
            let bottom_values = ctx.values(bottom);
            AreaChart {
                values: &top_values,
                max,
                gradient: top.1,
                mirrored: false,
                theme: ctx.theme,
            }
            .render(Rect::new(chart.x, chart.y, chart.width, top_h), buf);
            AreaChart {
                values: &bottom_values,
                max,
                gradient: bottom_grad,
                mirrored: true,
                theme: ctx.theme,
            }
            .render(
                Rect::new(chart.x, chart.y + top_h, chart.width, chart.height - top_h),
                buf,
            );
        }
        _ => AreaChart {
            values: &top_values,
            max,
            gradient: top.1,
            mirrored: false,
            theme: ctx.theme,
        }
        .render(chart, buf),
    }
}
