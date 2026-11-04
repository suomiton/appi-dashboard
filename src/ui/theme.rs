//! Colors: the btop-style palette, gradients, and 24-bit → 256-color fallback.

use ratatui::style::{Color, Style};

use crate::config::ColorMode;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

/// Exact values from the approved mockup.
pub mod palette {
    use super::Rgb;
    pub const BG: Rgb = Rgb(0x0f, 0x11, 0x1a);
    pub const BORDER: Rgb = Rgb(0x2f, 0x36, 0x54);
    pub const TEXT: Rgb = Rgb(0xc0, 0xca, 0xf5);
    pub const SOFT: Rgb = Rgb(0xa9, 0xb1, 0xd6);
    pub const DIM: Rgb = Rgb(0x56, 0x5f, 0x89);
    pub const BLUE: Rgb = Rgb(0x7a, 0xa2, 0xf7);
    pub const CYAN: Rgb = Rgb(0x7d, 0xcf, 0xff);
    pub const TEAL: Rgb = Rgb(0x73, 0xda, 0xca);
    pub const GREEN: Rgb = Rgb(0x9e, 0xce, 0x6a);
    pub const YELLOW: Rgb = Rgb(0xe0, 0xaf, 0x68);
    pub const ORANGE: Rgb = Rgb(0xff, 0x9e, 0x64);
    pub const RED: Rgb = Rgb(0xf7, 0x76, 0x8e);
    pub const PURPLE: Rgb = Rgb(0xbb, 0x9a, 0xf7);
    pub const MAGENTA: Rgb = Rgb(0xff, 0x00, 0x7c);
    pub const SEL_BG: Rgb = Rgb(0x28, 0x34, 0x57);
    pub const METER_OFF: Rgb = Rgb(0x25, 0x2a, 0x40);
    pub const SKY: Rgb = Rgb(0x2a, 0xc3, 0xde);
}

use palette::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gradient {
    Load,
    Cool,
    Hot,
    Violet,
    Teal,
    Lime,
    Ok,
}

impl Gradient {
    pub fn stops(self) -> &'static [Rgb] {
        match self {
            Gradient::Load => &[GREEN, YELLOW, RED],
            Gradient::Cool => &[SKY, BLUE, PURPLE],
            Gradient::Hot => &[YELLOW, ORANGE, RED],
            Gradient::Violet => &[BLUE, PURPLE, MAGENTA],
            Gradient::Teal => &[TEAL, CYAN, BLUE],
            Gradient::Lime => &[GREEN, TEAL, YELLOW],
            Gradient::Ok => &[GREEN, GREEN],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Depth {
    TrueColor,
    Ansi256,
}

/// Guesses whether the terminal supports 24-bit color from its environment.
pub fn detect_depth(get: impl Fn(&str) -> Option<String>) -> Depth {
    let has = |k: &str| get(k).is_some_and(|v| !v.is_empty());
    let colorterm = get("COLORTERM").unwrap_or_default().to_lowercase();
    if colorterm == "truecolor" || colorterm == "24bit" {
        return Depth::TrueColor;
    }
    if has("WT_SESSION") {
        return Depth::TrueColor;
    }
    let program = get("TERM_PROGRAM").unwrap_or_default();
    if program == "Apple_Terminal" {
        return Depth::Ansi256;
    }
    if ["iTerm.app", "WezTerm", "vscode", "ghostty"].contains(&program.as_str()) {
        return Depth::TrueColor;
    }
    let term = get("TERM").unwrap_or_default().to_lowercase();
    if ["kitty", "alacritty", "foot", "ghostty", "wezterm"]
        .iter()
        .any(|t| term.contains(t))
    {
        return Depth::TrueColor;
    }
    Depth::Ansi256
}

pub fn resolve_depth(mode: ColorMode, detected: Depth) -> Depth {
    match mode {
        ColorMode::Auto => detected,
        ColorMode::Truecolor => Depth::TrueColor,
        ColorMode::Ansi256 => Depth::Ansi256,
    }
}

/// Nearest xterm-256 color: the 6×6×6 cube or the 24-step gray ramp.
pub fn rgb_to_ansi256(c: Rgb) -> u8 {
    const LEVELS: [i32; 6] = [0, 95, 135, 175, 215, 255];
    let nearest = |v: u8| {
        LEVELS
            .iter()
            .enumerate()
            .min_by_key(|(_, l)| (**l - v as i32).abs())
            .map(|(i, _)| i)
            .unwrap_or(0)
    };
    let dist = |a: (i32, i32, i32)| {
        let (r, g, b) = (c.0 as i32 - a.0, c.1 as i32 - a.1, c.2 as i32 - a.2);
        r * r + g * g + b * b
    };
    let (ri, gi, bi) = (nearest(c.0), nearest(c.1), nearest(c.2));
    let cube = (LEVELS[ri], LEVELS[gi], LEVELS[bi]);
    let cube_index = 16 + 36 * ri + 6 * gi + bi;

    let avg = (c.0 as i32 + c.1 as i32 + c.2 as i32) / 3;
    let gray_step = ((avg - 8 + 5) / 10).clamp(0, 23);
    let gray = 8 + 10 * gray_step;
    let gray_index = 232 + gray_step as usize;

    if dist((gray, gray, gray)) < dist(cube) {
        gray_index as u8
    } else {
        cube_index as u8
    }
}

/// Linear interpolation along gradient stops; `t` is clamped to 0..=1.
pub fn lerp(stops: &[Rgb], t: f64) -> Rgb {
    match stops {
        [] => TEXT,
        [only] => *only,
        _ => {
            let t = if t.is_finite() {
                t.clamp(0.0, 1.0)
            } else {
                0.0
            };
            let x = t * (stops.len() - 1) as f64;
            let i = (x.floor() as usize).min(stops.len() - 2);
            let f = x - i as f64;
            let (a, b) = (stops[i], stops[i + 1]);
            let mix = |p: u8, q: u8| (p as f64 + (q as f64 - p as f64) * f).round() as u8;
            Rgb(mix(a.0, b.0), mix(a.1, b.1), mix(a.2, b.2))
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub depth: Depth,
}

impl Theme {
    pub fn new(depth: Depth) -> Self {
        Self { depth }
    }

    pub fn color(&self, c: Rgb) -> Color {
        match self.depth {
            Depth::TrueColor => Color::Rgb(c.0, c.1, c.2),
            Depth::Ansi256 => Color::Indexed(rgb_to_ansi256(c)),
        }
    }

    pub fn grad(&self, g: Gradient, t: f64) -> Color {
        self.color(lerp(g.stops(), t))
    }

    pub fn fg(&self, c: Rgb) -> Style {
        Style::new().fg(self.color(c)).bg(self.color(BG))
    }

    /// The app's own background and default text color.
    pub fn base(&self) -> Style {
        self.fg(TEXT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let m: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k| m.get(k).cloned()
    }

    #[test]
    fn detects_depth() {
        assert_eq!(
            detect_depth(env(&[("COLORTERM", "truecolor")])),
            Depth::TrueColor
        );
        assert_eq!(
            detect_depth(env(&[("COLORTERM", "24bit")])),
            Depth::TrueColor
        );
        assert_eq!(
            detect_depth(env(&[
                ("TERM_PROGRAM", "Apple_Terminal"),
                ("TERM", "xterm-256color")
            ])),
            Depth::Ansi256
        );
        assert_eq!(detect_depth(env(&[("WT_SESSION", "1")])), Depth::TrueColor);
        assert_eq!(
            detect_depth(env(&[("TERM_PROGRAM", "iTerm.app")])),
            Depth::TrueColor
        );
        assert_eq!(
            detect_depth(env(&[("TERM", "xterm-kitty")])),
            Depth::TrueColor
        );
        assert_eq!(
            detect_depth(env(&[("TERM", "xterm-256color")])),
            Depth::Ansi256
        );
        assert_eq!(detect_depth(env(&[])), Depth::Ansi256);
    }

    #[test]
    fn maps_to_xterm_256() {
        assert_eq!(rgb_to_ansi256(Rgb(0, 0, 0)), 16);
        assert_eq!(rgb_to_ansi256(Rgb(255, 255, 255)), 231);
        assert_eq!(rgb_to_ansi256(Rgb(255, 0, 0)), 196);
        assert_eq!(rgb_to_ansi256(Rgb(128, 128, 128)), 244);
        assert_eq!(rgb_to_ansi256(Rgb(15, 17, 26)), 233);
    }

    #[test]
    fn gradients_interpolate() {
        assert_eq!(lerp(Gradient::Load.stops(), 0.0), GREEN);
        assert_eq!(lerp(Gradient::Load.stops(), 1.0), RED);
        assert_eq!(lerp(Gradient::Load.stops(), 0.5), YELLOW);
        assert_eq!(lerp(Gradient::Ok.stops(), 0.7), GREEN);
        assert_eq!(lerp(Gradient::Load.stops(), -3.0), GREEN);
        assert_eq!(lerp(Gradient::Load.stops(), 9.0), RED);
    }

    #[test]
    fn theme_colors_follow_depth() {
        assert!(matches!(
            Theme::new(Depth::Ansi256).color(RED),
            Color::Indexed(_)
        ));
        assert_eq!(
            Theme::new(Depth::TrueColor).color(RED),
            Color::Rgb(0xf7, 0x76, 0x8e)
        );
        assert_eq!(
            Theme::new(Depth::TrueColor).base().bg,
            Some(Color::Rgb(0x0f, 0x11, 0x1a))
        );
    }

    #[test]
    fn resolves_configured_depth() {
        assert_eq!(
            resolve_depth(ColorMode::Truecolor, Depth::Ansi256),
            Depth::TrueColor
        );
        assert_eq!(
            resolve_depth(ColorMode::Ansi256, Depth::TrueColor),
            Depth::Ansi256
        );
        assert_eq!(
            resolve_depth(ColorMode::Auto, Depth::TrueColor),
            Depth::TrueColor
        );
    }
}
