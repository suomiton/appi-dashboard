//! btop-style ■ meters.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::Widget;

use crate::ui::theme::{Gradient, Theme, palette};

pub struct Meter<'a> {
    pub ratio: f64,
    pub gradient: Gradient,
    pub theme: &'a Theme,
}

impl Widget for Meter<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let area = area.intersection(buf.area);
        let n = area.width as usize;
        if n == 0 || area.height == 0 {
            return;
        }
        let ratio = if self.ratio.is_finite() {
            self.ratio.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let lit = (ratio * n as f64).round() as usize;
        let bg = self.theme.color(palette::BG);
        for i in 0..n {
            let fg = if i < lit {
                self.theme.grad(
                    self.gradient,
                    if n > 1 {
                        i as f64 / (n - 1) as f64
                    } else {
                        0.0
                    },
                )
            } else {
                self.theme.color(palette::METER_OFF)
            };
            buf.set_string(area.x + i as u16, area.y, "■", Style::new().fg(fg).bg(bg));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::theme::{Depth, Gradient, Theme, palette};
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::widgets::Widget;

    #[test]
    fn lights_ratio_of_cells() {
        let theme = Theme::new(Depth::TrueColor);
        let area = Rect::new(0, 0, 10, 1);
        let mut buf = Buffer::empty(area);
        Meter {
            ratio: 0.3,
            gradient: Gradient::Load,
            theme: &theme,
        }
        .render(area, &mut buf);
        let off = theme.color(palette::METER_OFF);
        for x in 0..10 {
            assert_eq!(buf[(x, 0)].symbol(), "■");
            assert_eq!(buf[(x, 0)].fg == off, x >= 3, "cell {x}");
        }
    }

    #[test]
    fn clamps_ratio() {
        let theme = Theme::new(Depth::TrueColor);
        let area = Rect::new(0, 0, 4, 1);
        let mut buf = Buffer::empty(area);
        Meter {
            ratio: 7.0,
            gradient: Gradient::Ok,
            theme: &theme,
        }
        .render(area, &mut buf);
        assert_ne!(buf[(3, 0)].fg, theme.color(palette::METER_OFF));
        let mut buf = Buffer::empty(area);
        Meter {
            ratio: f64::NAN,
            gradient: Gradient::Ok,
            theme: &theme,
        }
        .render(area, &mut buf);
        assert_eq!(buf[(0, 0)].fg, theme.color(palette::METER_OFF));
    }
}
