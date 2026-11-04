//! Filled braille area charts (btop style) and block sparklines.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::Widget;

use crate::data::time::resample;
use crate::ui::theme::{Gradient, Theme, palette};

const LEFT_BITS: [u32; 4] = [0x01, 0x02, 0x04, 0x40];
const RIGHT_BITS: [u32; 4] = [0x08, 0x10, 0x20, 0x80];

fn dots(v: f64, height: usize) -> usize {
    let total = height * 4;
    if v.is_nan() || v <= 0.0 {
        return 0;
    }
    let n = (v.min(1.0) * total as f64).round() as usize;
    n.max(1)
}

/// Braille rows (top first) for normalized samples, two per character
/// column. Non-zero samples always show at least one dot.
pub fn braille_rows(samples: &[f64], height: usize, mirrored: bool) -> Vec<String> {
    let cols = samples.len().div_ceil(2);
    let sample = |i: usize| samples.get(i).copied().unwrap_or(0.0);
    (0..height)
        .map(|r| {
            (0..cols)
                .map(|c| {
                    let (nl, nr) = (dots(sample(2 * c), height), dots(sample(2 * c + 1), height));
                    let mut bits = 0u32;
                    for d in 0..4 {
                        let k = if mirrored {
                            r * 4 + d
                        } else {
                            (height - 1 - r) * 4 + (3 - d)
                        };
                        if k < nl {
                            bits |= LEFT_BITS[d];
                        }
                        if k < nr {
                            bits |= RIGHT_BITS[d];
                        }
                    }
                    char::from_u32(0x2800 + bits).unwrap_or(' ')
                })
                .collect()
        })
        .collect()
}

/// A filled area chart; `mirrored` grows downward from the top edge.
pub struct AreaChart<'a> {
    pub values: &'a [Option<f64>],
    pub max: Option<f64>,
    pub gradient: Gradient,
    pub mirrored: bool,
    pub theme: &'a Theme,
}

impl Widget for AreaChart<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let area = area.intersection(buf.area);
        if area.width == 0 || area.height == 0 {
            return;
        }
        let samples: Vec<f64> = resample(self.values, area.width as usize * 2)
            .into_iter()
            .map(|v| v.filter(|x| x.is_finite()).unwrap_or(0.0).max(0.0))
            .collect();
        let peak = samples.iter().copied().fold(0.0, f64::max);
        let max =
            self.max
                .filter(|m| *m > 0.0)
                .unwrap_or(if peak > 0.0 { peak * 1.1 } else { 1.0 });
        let normalized: Vec<f64> = samples.iter().map(|v| v / max).collect();
        let h = area.height as usize;
        let bg = self.theme.color(palette::BG);
        for (r, line) in braille_rows(&normalized, h, self.mirrored)
            .iter()
            .enumerate()
        {
            let t = if h < 2 {
                0.5
            } else if self.mirrored {
                r as f64 / (h - 1) as f64
            } else {
                (h - 1 - r) as f64 / (h - 1) as f64
            };
            let style = ratatui::style::Style::new()
                .fg(self.theme.grad(self.gradient, t))
                .bg(bg);
            buf.set_string(area.x, area.y + r as u16, line, style);
        }
    }
}

const SPARK: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// A one-line block sparkline, resampled to `width` cells.
pub fn sparkline(values: &[f64], width: usize) -> String {
    let opt: Vec<Option<f64>> = values.iter().map(|v| Some(*v)).collect();
    let cells: Vec<f64> = resample(&opt, width)
        .into_iter()
        .map(|v| v.filter(|x| x.is_finite()).unwrap_or(0.0).max(0.0))
        .collect();
    let max = cells.iter().copied().fold(0.0, f64::max);
    cells
        .iter()
        .map(|v| {
            if max <= 0.0 {
                SPARK[0]
            } else {
                SPARK[((v / max) * 7.0).round().clamp(0.0, 7.0) as usize]
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::theme::{Depth, Theme};
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::widgets::Widget;

    #[test]
    fn braille_cells() {
        assert_eq!(braille_rows(&[1.0, 1.0], 1, false), vec!["⣿"]);
        assert_eq!(braille_rows(&[0.0, 0.0], 1, false), vec!["\u{2800}"]);
        assert_eq!(braille_rows(&[0.25, 0.0], 1, false), vec!["⡀"]);
        assert_eq!(braille_rows(&[0.25, 0.0], 1, true), vec!["⠁"]);
        assert_eq!(braille_rows(&[0.5, 0.5], 2, false), vec!["\u{2800}", "⣿"]);
        assert_eq!(braille_rows(&[0.0, 0.25], 1, false), vec!["⢀"]);
    }

    #[test]
    fn tiny_non_zero_values_stay_visible() {
        let rows = braille_rows(&[0.01, 0.0], 4, false);
        assert_eq!(rows[3], "⡀");
        assert_eq!(rows[0], "\u{2800}");
    }

    #[test]
    fn odd_sample_count_is_padded() {
        assert_eq!(braille_rows(&[1.0], 1, false), vec!["⡇"]);
    }

    #[test]
    fn sparklines() {
        assert_eq!(sparkline(&[0., 1.], 2), "▁█");
        assert_eq!(sparkline(&[0., 0., 0.], 3), "▁▁▁");
        assert_eq!(sparkline(&[], 2), "▁▁");
    }

    #[test]
    fn chart_renders_without_data() {
        let theme = Theme::new(Depth::TrueColor);
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 3));
        AreaChart {
            values: &[None, None, None],
            max: None,
            gradient: crate::ui::theme::Gradient::Hot,
            mirrored: false,
            theme: &theme,
        }
        .render(Rect::new(0, 0, 10, 3), &mut buf);
        assert_eq!(buf[(0, 2)].symbol(), "\u{2800}");
    }

    #[test]
    fn chart_fills_from_bottom_and_colors_rows() {
        let theme = Theme::new(Depth::TrueColor);
        let mut buf = Buffer::empty(Rect::new(0, 0, 2, 2));
        AreaChart {
            values: &[Some(1.0), Some(1.0)],
            max: Some(1.0),
            gradient: crate::ui::theme::Gradient::Load,
            mirrored: false,
            theme: &theme,
        }
        .render(Rect::new(0, 0, 2, 2), &mut buf);
        assert_eq!(buf[(0, 0)].symbol(), "⣿");
        assert_eq!(buf[(1, 1)].symbol(), "⣿");
        assert_ne!(
            buf[(0, 0)].fg,
            buf[(0, 1)].fg,
            "top and bottom rows use different gradient colors"
        );
    }
}
