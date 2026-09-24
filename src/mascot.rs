//! FLOE, ICE's isometric pixel ice cube, plus the ICE wordmark — drawn with
//! coloured cells so the home screen looks the same in any terminal.

use crate::render::{Line, Span, Style};
use crate::theme::Theme;

pub const WORDMARK: [&str; 5] = [
    "███  ██████  ███████",
    " █   ██      ██     ",
    " █   ██      █████  ",
    " █   ██      ██     ",
    "███  ██████  ███████",
];

const SPRITE: [&str; 11] = [
    "      TTTTTTTTTT    ",
    "    TTHHHHHHHHTTS   ",
    "  TTHHHHHHHHTTSSS   ",
    "TTTTTTTTTTTTSSSSS   ",
    "FFFFFFFFFFFFSSSSS   ",
    "FHHFFFFFFFFFSSSSS   ",
    "FHHFFFFFFFFFSSSSS   ",
    "FFFFFFFFFFFFSSSSS   ",
    "FFFFFFFFFFFFSSSS    ",
    "FFFFFFFFFFFFSS      ",
    "FFFFFFFFFFFF        ",
];

/// Width of `hero()` rows in cells.
pub const HERO_WIDTH: usize = 50;

/// The big FLOE: 12 rows × 50 cells (sprite, companion, shadow).
pub fn hero(t: &Theme) -> Vec<Line> {
    let mut rows = Vec::new();
    for (r, px) in SPRITE.iter().enumerate() {
        // Two cells per pixel, as (text, style) pairs.
        let mut cells: Vec<(String, Style)> = px
            .chars()
            .map(|p| {
                let bg = match p {
                    'T' => Some(t.floe_top),
                    'H' => Some(t.highlight),
                    'F' => Some(t.floe_face),
                    'S' => Some(t.floe_side),
                    _ => None,
                };
                (
                    "  ".to_string(),
                    Style {
                        bg,
                        ..Default::default()
                    },
                )
            })
            .collect();
        let face = Style {
            fg: Some(t.bg),
            bg: Some(t.floe_face),
            ..Default::default()
        };
        if r == 6 {
            // Eyes.
            cells[3] = ("▐▌".into(), face);
            cells[8] = ("▐▌".into(), face);
        }
        if r == 8 {
            cells[5] = ("▄▄".into(), face);
        }
        let mut l = Line::raw("  ");
        for (text, st) in cells {
            l.spans.push(Span { text, style: st });
        }
        // Companion mini-floe, bobbing beside the big one.
        let side = match r {
            4 => " ▄▄▄▄ ",
            5 => "█▪▪██▀",
            6 => "▀▀▀▀  ",
            _ => "      ",
        };
        l.add("  ", Style::default());
        l.add(side, Style::fg(t.floe_top));
        rows.push(l);
    }
    rows.push(Line::styled("      ░░▒▒▒▒▒▒▒▒▒▒▒▒░░", Style::fg(t.border)));
    for l in rows.iter_mut() {
        let pad = HERO_WIDTH.saturating_sub(l.width());
        l.add(" ".repeat(pad), Style::default());
    }
    rows
}

pub fn wordmark(t: &Theme) -> Vec<Line> {
    WORDMARK
        .iter()
        .map(|s| Line::styled(*s, Style::fg(t.floe_top)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hero_rows_have_equal_width() {
        let t = crate::theme::ThemeName::DEFAULT.palette();
        let h = hero(&t);
        assert_eq!(h.len(), 12);
        for l in &h {
            assert_eq!(l.width(), HERO_WIDTH, "{:?}", l.text());
        }
        assert_eq!(wordmark(&t).len(), 5);
    }
}
