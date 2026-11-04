//! Three-row block-character digits for headline numbers.

fn glyph(c: char) -> [&'static str; 3] {
    match c {
        '0' => ["█▀█", "█ █", "▀▀▀"],
        '1' => ["▀█ ", " █ ", "▀▀▀"],
        '2' => ["▀▀█", "█▀▀", "▀▀▀"],
        '3' => ["▀▀█", " ▀█", "▀▀▀"],
        '4' => ["█ █", "▀▀█", "  ▀"],
        '5' => ["█▀▀", "▀▀█", "▀▀▀"],
        '6' => ["█▀▀", "█▀█", "▀▀▀"],
        '7' => ["▀▀█", "  █", "  ▀"],
        '8' => ["█▀█", "█▀█", "▀▀▀"],
        '9' => ["█▀█", "▀▀█", "▀▀▀"],
        '.' => [" ", " ", "▀"],
        '-' => ["   ", "▀▀▀", "   "],
        _ => ["  ", "  ", "  "],
    }
}

/// Renders `s` as three rows of block characters, glyphs separated by a space.
pub fn big_text(s: &str) -> [String; 3] {
    let mut rows = [String::new(), String::new(), String::new()];
    for (i, c) in s.chars().enumerate() {
        let g = glyph(c);
        for (row, part) in rows.iter_mut().zip(g) {
            if i > 0 {
                row.push(' ');
            }
            row.push_str(part);
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digits() {
        assert_eq!(
            big_text("10"),
            ["▀█  █▀█".to_string(), " █  █ █".into(), "▀▀▀ ▀▀▀".into()]
        );
        assert_eq!(
            big_text("1.5"),
            [
                "▀█    █▀▀".to_string(),
                " █    ▀▀█".into(),
                "▀▀▀ ▀ ▀▀▀".into()
            ]
        );
        assert_eq!(big_text(""), [String::new(), String::new(), String::new()]);
        assert_eq!(big_text("x"), ["  ".to_string(), "  ".into(), "  ".into()]);
    }
}
