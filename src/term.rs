//! Inline terminal renderer (the Ink model Claude Code uses): finished
//! output is printed once into normal scrollback, and only the small "live"
//! region at the bottom (spinner, input box, dialogs) is redrawn in place.
//! Text stays selectable and scrollable with the terminal's own scrollbar.

use crate::render::{Line, Style};
use crate::theme;
use crossterm::cursor::{Hide, MoveToColumn, MoveUp, Show};
use crossterm::style::{
    Attribute, Color, Print, ResetColor, SetAttribute, SetBackgroundColor, SetForegroundColor,
};
use crossterm::terminal::{Clear, ClearType};
use crossterm::{queue, QueueableCommand};
use std::io::{self, BufWriter, Stdout, Write};

pub struct Screen {
    out: BufWriter<Stdout>,
    /// Rows occupied by the live region as last drawn.
    live_rows: usize,
    /// Row (within the live region) where the cursor was left.
    cursor_row: usize,
    truecolor: bool,
    no_color: bool,
    pub width: usize,
    pub height: usize,
}

impl Screen {
    pub fn new() -> Screen {
        let (w, h) = crossterm::terminal::size().unwrap_or((80, 24));
        Screen {
            out: BufWriter::with_capacity(1 << 16, io::stdout()),
            live_rows: 0,
            cursor_row: 0,
            truecolor: theme::truecolor(),
            no_color: theme::no_color(),
            width: w.max(20) as usize,
            height: h.max(8) as usize,
        }
    }

    pub fn refresh_size(&mut self) {
        let (w, h) = crossterm::terminal::size().unwrap_or((80, 24));
        self.width = w.max(20) as usize;
        self.height = h.max(8) as usize;
    }

    /// Usable content width (one column spare avoids auto-wrap quirks).
    pub fn cols(&self) -> usize {
        self.width.saturating_sub(1).max(19)
    }

    fn color(&self, c: Color) -> Color {
        if self.truecolor {
            c
        } else {
            theme::to_ansi256(c)
        }
    }

    fn write_line(&mut self, l: &Line) -> io::Result<()> {
        let mut used = 0;
        for s in &l.spans {
            self.style(&s.style)?;
            self.out.queue(Print(&s.text))?;
            used += crate::render::str_w(&s.text);
            self.out.queue(SetAttribute(Attribute::Reset))?;
            self.out.queue(ResetColor)?;
        }
        if let Some(bg) = l.fill {
            if !self.no_color && used < self.cols() {
                let c = self.color(bg);
                self.out.queue(SetBackgroundColor(c))?;
                self.out.queue(Print(" ".repeat(self.cols() - used)))?;
                self.out.queue(ResetColor)?;
            }
        }
        Ok(())
    }

    fn style(&mut self, s: &Style) -> io::Result<()> {
        if !self.no_color {
            if let Some(fg) = s.fg {
                if fg != Color::Reset {
                    let c = self.color(fg);
                    self.out.queue(SetForegroundColor(c))?;
                }
            }
            if let Some(bg) = s.bg {
                let c = self.color(bg);
                self.out.queue(SetBackgroundColor(c))?;
            }
        }
        if s.bold {
            self.out.queue(SetAttribute(Attribute::Bold))?;
        }
        if s.dim {
            self.out.queue(SetAttribute(Attribute::Dim))?;
        }
        if s.italic {
            self.out.queue(SetAttribute(Attribute::Italic))?;
        }
        if s.underline {
            self.out.queue(SetAttribute(Attribute::Underlined))?;
        }
        if s.strike {
            self.out.queue(SetAttribute(Attribute::CrossedOut))?;
        }
        Ok(())
    }

    fn clear_live(&mut self) -> io::Result<()> {
        if self.cursor_row > 0 {
            queue!(self.out, MoveUp(self.cursor_row as u16))?;
        }
        queue!(self.out, MoveToColumn(0), Clear(ClearType::FromCursorDown))?;
        self.live_rows = 0;
        self.cursor_row = 0;
        Ok(())
    }

    fn sync_begin(&mut self) -> io::Result<()> {
        self.out.write_all(b"\x1b[?2026h")
    }

    fn sync_end(&mut self) -> io::Result<()> {
        self.out.write_all(b"\x1b[?2026l")?;
        self.out.flush()
    }

    /// Print finished lines into scrollback, above the live region.
    pub fn commit(&mut self, lines: &[Line]) -> io::Result<()> {
        if lines.is_empty() {
            return Ok(());
        }
        self.sync_begin()?;
        queue!(self.out, Hide)?;
        self.clear_live()?;
        for l in lines {
            self.write_line(l)?;
            self.out.write_all(b"\r\n")?;
        }
        self.sync_end()
    }

    /// Redraw the live region. `cursor` = (row, col) to park the caret.
    pub fn draw_live(&mut self, lines: &[Line], cursor: Option<(usize, usize)>) -> io::Result<()> {
        self.sync_begin()?;
        queue!(self.out, Hide)?;
        self.clear_live()?;
        // Never taller than the screen, or the relative moves break.
        let max = self.height.saturating_sub(1).max(1);
        let skip = lines.len().saturating_sub(max);
        let shown = &lines[skip..];
        for (i, l) in shown.iter().enumerate() {
            if i > 0 {
                self.out.write_all(b"\r\n")?;
            }
            self.write_line(l)?;
        }
        self.live_rows = shown.len();
        let last = shown.len().saturating_sub(1);
        self.cursor_row = last;
        if let Some((r, c)) = cursor {
            if r >= skip {
                let r = r - skip;
                if last > r {
                    queue!(self.out, MoveUp((last - r) as u16))?;
                }
                queue!(self.out, MoveToColumn(c as u16), Show)?;
                self.cursor_row = r;
            }
        }
        self.sync_end()
    }

    /// Wipe screen and scrollback (for /clear, resize and transcript mode).
    pub fn clear_all(&mut self) -> io::Result<()> {
        self.out.write_all(b"\x1b[2J\x1b[3J\x1b[H")?;
        self.live_rows = 0;
        self.cursor_row = 0;
        self.out.flush()
    }

    /// Leave the live region in place and move below it (on exit).
    pub fn finish(&mut self) -> io::Result<()> {
        let down = self
            .live_rows
            .saturating_sub(1)
            .saturating_sub(self.cursor_row);
        if down > 0 {
            queue!(self.out, crossterm::cursor::MoveDown(down as u16))?;
        }
        queue!(self.out, MoveToColumn(0), Show)?;
        self.out.write_all(b"\r\n")?;
        self.out.flush()
    }
}
