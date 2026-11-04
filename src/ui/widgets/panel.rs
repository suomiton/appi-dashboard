//! Rounded btop-style panels with a hotkey number in the title.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Widget};

use crate::ui::theme::{Theme, palette};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Emphasis {
    Normal,
    Focused,
    Alert,
    AlertFocused,
}

pub struct PanelBlock<'a> {
    pub title: &'a str,
    pub hotkey: Option<char>,
    pub right: Option<Line<'a>>,
    pub emphasis: Emphasis,
    pub stale: Option<String>,
    pub theme: &'a Theme,
}

pub fn superscript(c: char) -> char {
    match c {
        '0' => '⁰',
        '1' => '¹',
        '2' => '²',
        '3' => '³',
        '4' => '⁴',
        '5' => '⁵',
        '6' => '⁶',
        '7' => '⁷',
        '8' => '⁸',
        '9' => '⁹',
        other => other,
    }
}

impl PanelBlock<'_> {
    /// The area inside the border.
    pub fn inner(area: Rect) -> Rect {
        Block::new().borders(Borders::ALL).inner(area)
    }

    pub fn render(&self, area: Rect, buf: &mut Buffer) {
        let area = area.intersection(buf.area);
        if area.width == 0 || area.height == 0 {
            return;
        }
        let t = self.theme;
        let alert = matches!(self.emphasis, Emphasis::Alert | Emphasis::AlertFocused);
        let border = match self.emphasis {
            Emphasis::Normal => t.fg(palette::BORDER),
            Emphasis::Focused => t.fg(palette::BLUE),
            Emphasis::Alert => t.fg(palette::RED),
            Emphasis::AlertFocused => t.fg(palette::RED).add_modifier(Modifier::BOLD),
        };
        let mut title = vec![Span::styled(" ", t.base())];
        if let Some(k) = self.hotkey {
            title.push(Span::styled(
                superscript(k).to_string(),
                t.fg(palette::ORANGE),
            ));
        }
        let title_style = if alert {
            t.fg(palette::RED)
        } else {
            t.fg(palette::TEXT)
        };
        title.push(Span::styled(self.title.to_string(), title_style));
        title.push(Span::styled(" ", t.base()));

        let mut block = Block::new()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(border)
            .style(t.base())
            .title_top(Line::from(title));
        let mut right: Vec<Span> = Vec::new();
        if let Some(r) = &self.right {
            right.push(Span::styled(" ", t.base()));
            right.extend(r.spans.iter().cloned());
        }
        if let Some(age) = &self.stale {
            if right.is_empty() {
                right.push(Span::styled(" ", t.base()));
            }
            right.push(Span::styled(
                format!(" · stale {age}"),
                t.fg(palette::YELLOW),
            ));
        }
        if !right.is_empty() {
            right.push(Span::styled(" ", t.base()));
            block = block.title_top(Line::from(right).right_aligned());
        }
        Widget::render(block, area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::theme::{Depth, Theme, palette};

    fn row(buf: &Buffer, y: u16) -> String {
        (0..buf.area.width)
            .map(|x| buf[(x, y)].symbol().to_string())
            .collect()
    }

    #[test]
    fn alert_panel_has_red_rounded_border_and_title() {
        let theme = Theme::new(Depth::TrueColor);
        let area = Rect::new(0, 0, 30, 5);
        let mut buf = Buffer::empty(area);
        PanelBlock {
            title: "failed requests",
            hotkey: Some('2'),
            right: None,
            emphasis: Emphasis::Alert,
            stale: None,
            theme: &theme,
        }
        .render(area, &mut buf);
        assert_eq!(buf[(0, 0)].symbol(), "╭");
        assert_eq!(buf[(0, 0)].fg, theme.color(palette::RED));
        assert!(
            row(&buf, 0).contains("²failed requests"),
            "{}",
            row(&buf, 0)
        );
        assert_eq!(PanelBlock::inner(area), Rect::new(1, 1, 28, 3));
    }

    #[test]
    fn focused_border_is_blue_and_stale_is_shown() {
        let theme = Theme::new(Depth::TrueColor);
        let area = Rect::new(0, 0, 40, 4);
        let mut buf = Buffer::empty(area);
        PanelBlock {
            title: "cpu",
            hotkey: None,
            right: Some(Line::from("process")),
            emphasis: Emphasis::Focused,
            stale: Some("3m".into()),
            theme: &theme,
        }
        .render(area, &mut buf);
        assert_eq!(buf[(0, 0)].fg, theme.color(palette::BLUE));
        assert!(
            row(&buf, 0).contains("process · stale 3m"),
            "{}",
            row(&buf, 0)
        );
    }

    #[test]
    fn tiny_areas_do_not_panic() {
        let theme = Theme::new(Depth::Ansi256);
        for (w, h) in [(0, 0), (1, 1), (2, 2), (3, 1)] {
            let area = Rect::new(0, 0, w, h);
            let mut buf = Buffer::empty(area);
            PanelBlock {
                title: "a very long title that does not fit",
                hotkey: Some('9'),
                right: Some(Line::from("right side")),
                emphasis: Emphasis::Normal,
                stale: None,
                theme: &theme,
            }
            .render(area, &mut buf);
            let _ = PanelBlock::inner(area);
        }
    }

    #[test]
    fn superscripts() {
        assert_eq!(superscript('7'), '⁷');
        assert_eq!(superscript('0'), '⁰');
        assert_eq!(superscript('x'), 'x');
    }
}
