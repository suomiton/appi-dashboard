//! The instance picker overlay.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::{Clear, Widget};

use crate::app::PickerState;
use crate::data::format::truncate_width;
use crate::ui::common::{Ctx, put_line};
use crate::ui::layout::centered;
use crate::ui::theme::palette;
use crate::ui::widgets::panel::{Emphasis, PanelBlock};

pub fn render(buf: &mut Buffer, area: Rect, ctx: &Ctx, p: &PickerState) {
    let w = (area.width as u32 * 80 / 100) as u16;
    let h = (area.height as u32 * 70 / 100) as u16;
    let box_area = centered(area, w.max(40), h.max(8));
    Clear.render(box_area, buf);
    let esc = if p.can_cancel {
        "esc cancel"
    } else {
        "esc quit"
    };
    PanelBlock {
        title: "select application insights",
        hotkey: None,
        right: None,
        emphasis: Emphasis::Focused,
        stale: None,
        theme: ctx.theme,
    }
    .render(box_area, buf);
    let inner = PanelBlock::inner(box_area).intersection(buf.area);
    if inner.height < 3 || inner.width < 10 {
        return;
    }
    put_line(
        buf,
        inner,
        0,
        &Line::from(vec![
            ctx.s(" › ", palette::BLUE),
            ctx.s(p.query.clone(), palette::TEXT),
            ctx.s("▏", palette::BLUE),
        ]),
    );
    let list_h = inner.height.saturating_sub(3);
    match &p.all {
        None => put_line(
            buf,
            inner,
            2,
            &Line::from(ctx.s(" discovering instances…", palette::DIM)),
        ),
        Some(Err(e)) => {
            put_line(
                buf,
                inner,
                2,
                &Line::from(ctx.s(format!(" {e}"), palette::RED)),
            );
        }
        Some(Ok(_)) => {
            let rows = p.filtered();
            if rows.is_empty() {
                put_line(
                    buf,
                    inner,
                    2,
                    &Line::from(ctx.s(" no matches", palette::DIM)),
                );
            }
            // Keep the selection visible when the list is longer than the box.
            let first = p.selected.saturating_sub(list_h.saturating_sub(1) as usize);
            for (i, inst) in rows.iter().enumerate().skip(first).take(list_h as usize) {
                let y = 2 + (i - first) as u16;
                let selected = i == p.selected;
                let name_w = (inner.width as usize / 2).max(10);
                let marker = if selected { "▶ " } else { "  " };
                let line = Line::from(vec![
                    ctx.s(marker, palette::BLUE),
                    ctx.s(
                        format!("{:<name_w$}", truncate_width(&inst.name, name_w - 1)),
                        palette::TEXT,
                    ),
                    ctx.s(
                        format!("{}  {}", inst.resource_group, inst.subscription_name),
                        palette::DIM,
                    ),
                ]);
                put_line(buf, inner, y, &line);
                if selected {
                    let row = Rect::new(inner.x, inner.y + y, inner.width, 1);
                    buf.set_style(
                        row,
                        ratatui::style::Style::new().bg(ctx.theme.color(palette::SEL_BG)),
                    );
                }
            }
        }
    }
    put_line(
        buf,
        inner,
        inner.height - 1,
        &Line::from(vec![
            ctx.s(" fuzzy ·", palette::DIM),
            ctx.s(" ↑↓", palette::ORANGE),
            ctx.s(" select  ", palette::DIM),
            ctx.s("↵", palette::ORANGE),
            ctx.s(" open  ", palette::DIM),
            ctx.s("ctrl+r", palette::ORANGE),
            ctx.s(" rediscover  ", palette::DIM),
            ctx.s(esc, palette::DIM),
        ]),
    );
}
