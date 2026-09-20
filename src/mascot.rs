//! FLOE: a terminal-native, shaded pixel ice companion.
use crate::theme::Theme;
use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

pub const WORDMARK: [&str; 5] = [
    "███  ██████  ███████",
    " █   ██      ██     ",
    " █   ██      █████  ",
    " █   ██      ██     ",
    "███  ██████  ███████",
];

pub fn draw(f: &mut Frame, area: Rect, t: Theme, tick: u64, busy: bool) {
    let sprite = [
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
    let bob = u16::from((tick / 7) % 4 == 1);
    let x = area.x + area.width.saturating_sub(38) / 2;
    let y = area.y + bob;
    for (row, pixels) in sprite.iter().enumerate() {
        for (col, pixel) in pixels.chars().enumerate() {
            let color = match pixel {
                'T' => t.ice,
                'H' => t.text,
                'F' => t.accent,
                'S' => t.accent_dim,
                _ => continue,
            };
            let px = x + col as u16 * 2;
            let py = y + row as u16;
            if px + 1 < area.right() && py < area.bottom() {
                f.render_widget(
                    Paragraph::new("  ").style(Style::default().bg(color)),
                    Rect::new(px, py, 2, 1),
                );
            }
        }
    }
    let eyes = if tick % 61 >= 58 { "▄▄" } else { "██" };
    let look = u16::from((tick / 24) % 3 == 1);
    for dx in [6, 16] {
        let eye = Rect::new(x + dx + look, y + 6, 2, 1);
        if eye.right() <= area.right() && eye.bottom() <= area.bottom() {
            f.render_widget(
                Paragraph::new(eyes).style(Style::default().fg(t.bg).bg(t.accent)),
                eye,
            );
        }
    }
    if y + 8 < area.bottom() && x + 14 < area.right() {
        f.render_widget(
            Paragraph::new(if busy { "▪▪" } else { "▀▀" })
                .style(Style::default().fg(t.bg).bg(t.accent)),
            Rect::new(x + 11, y + 8, 2, 1),
        );
    }
    if y + 12 < area.bottom() {
        f.render_widget(
            Paragraph::new("    ░░▒▒▒▒▒▒▒▒░░").style(Style::default().fg(t.border)),
            Rect::new(x, y + 12, area.right().saturating_sub(x), 1),
        );
    }
    if area.width >= 48 {
        let fx = area.right() - 8;
        let fy = area.y + 3 + u16::from((tick / 9) % 2 == 0);
        for (i, line) in [" ▄▄▄▄ ", "█▪▪██▀", "▀▀▀▀  "].iter().enumerate()
        {
            if fy + (i as u16) < area.bottom() {
                f.render_widget(
                    Paragraph::new(*line).style(Style::default().fg(t.ice)),
                    Rect::new(fx, fy + i as u16, 6, 1),
                );
            }
        }
    }
}

pub fn wordmark(t: Theme) -> Vec<Line<'static>> {
    WORDMARK
        .iter()
        .map(|s| Line::from(Span::styled(*s, Style::default().fg(t.ice))))
        .collect()
}
