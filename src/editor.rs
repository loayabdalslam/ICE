//! The prompt editor: a multi-line text buffer with readline-style editing
//! (Ctrl+A/E/U/K/W, Alt+B/F, word jumps) and soft-wrapping into the input
//! box with an exact caret position.

use crate::render::char_w;

#[derive(Clone, Debug, Default)]
pub struct Editor {
    pub text: String,
    /// Byte offset of the caret.
    pub cursor: usize,
}

impl Editor {
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn set(&mut self, t: &str) {
        self.text = t.to_string();
        self.cursor = self.text.len();
    }

    pub fn take(&mut self) -> String {
        self.cursor = 0;
        std::mem::take(&mut self.text)
    }

    pub fn insert(&mut self, c: char) {
        self.text.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    pub fn insert_str(&mut self, s: &str) {
        let s = s.replace("\r\n", "\n").replace('\r', "\n");
        self.text.insert_str(self.cursor, &s);
        self.cursor += s.len();
    }

    fn prev_boundary(&self, i: usize) -> usize {
        self.text[..i]
            .char_indices()
            .last()
            .map(|(k, _)| k)
            .unwrap_or(0)
    }

    fn next_boundary(&self, i: usize) -> usize {
        self.text[i..]
            .chars()
            .next()
            .map(|c| i + c.len_utf8())
            .unwrap_or(i)
    }

    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let p = self.prev_boundary(self.cursor);
        self.text.replace_range(p..self.cursor, "");
        self.cursor = p;
    }

    pub fn delete(&mut self) {
        if self.cursor >= self.text.len() {
            return;
        }
        let n = self.next_boundary(self.cursor);
        self.text.replace_range(self.cursor..n, "");
    }

    pub fn left(&mut self) {
        self.cursor = self.prev_boundary(self.cursor);
    }

    pub fn right(&mut self) {
        self.cursor = self.next_boundary(self.cursor);
    }

    fn is_word(c: char) -> bool {
        c.is_alphanumeric() || c == '_'
    }

    pub fn word_left(&mut self) {
        let mut i = self.cursor;
        while i > 0 {
            let p = self.prev_boundary(i);
            if Self::is_word(self.text[p..i].chars().next().unwrap()) {
                break;
            }
            i = p;
        }
        while i > 0 {
            let p = self.prev_boundary(i);
            if !Self::is_word(self.text[p..i].chars().next().unwrap()) {
                break;
            }
            i = p;
        }
        self.cursor = i;
    }

    pub fn word_right(&mut self) {
        let mut i = self.cursor;
        let len = self.text.len();
        while i < len && !Self::is_word(self.text[i..].chars().next().unwrap()) {
            i = self.next_boundary(i);
        }
        while i < len && Self::is_word(self.text[i..].chars().next().unwrap()) {
            i = self.next_boundary(i);
        }
        self.cursor = i;
    }

    pub fn delete_word_back(&mut self) {
        let end = self.cursor;
        self.word_left();
        self.text.replace_range(self.cursor..end, "");
    }

    fn line_start(&self) -> usize {
        self.text[..self.cursor]
            .rfind('\n')
            .map(|i| i + 1)
            .unwrap_or(0)
    }

    fn line_end(&self) -> usize {
        self.text[self.cursor..]
            .find('\n')
            .map(|i| self.cursor + i)
            .unwrap_or(self.text.len())
    }

    pub fn home(&mut self) {
        self.cursor = self.line_start();
    }

    pub fn end(&mut self) {
        self.cursor = self.line_end();
    }

    pub fn kill_to_start(&mut self) {
        let s = self.line_start();
        let s = if s == self.cursor && s > 0 { s - 1 } else { s };
        self.text.replace_range(s..self.cursor, "");
        self.cursor = s;
    }

    pub fn kill_to_end(&mut self) {
        let e = self.line_end();
        let e = if e == self.cursor && e < self.text.len() {
            e + 1
        } else {
            e
        };
        self.text.replace_range(self.cursor..e, "");
    }

    pub fn on_first_line(&self) -> bool {
        !self.text[..self.cursor].contains('\n')
    }

    pub fn on_last_line(&self) -> bool {
        !self.text[self.cursor..].contains('\n')
    }

    /// Move up a logical line keeping the column; false if already on top.
    pub fn up(&mut self) -> bool {
        if self.on_first_line() {
            return false;
        }
        let col = self.text[self.line_start()..self.cursor].chars().count();
        let cur_start = self.line_start();
        let prev_end = cur_start - 1;
        let prev_start = self.text[..prev_end]
            .rfind('\n')
            .map(|i| i + 1)
            .unwrap_or(0);
        self.cursor = self.text[prev_start..prev_end]
            .char_indices()
            .nth(col)
            .map(|(i, _)| prev_start + i)
            .unwrap_or(prev_end);
        true
    }

    pub fn down(&mut self) -> bool {
        if self.on_last_line() {
            return false;
        }
        let col = self.text[self.line_start()..self.cursor].chars().count();
        let next_start = self.line_end() + 1;
        let next_end = self.text[next_start..]
            .find('\n')
            .map(|i| next_start + i)
            .unwrap_or(self.text.len());
        self.cursor = self.text[next_start..next_end]
            .char_indices()
            .nth(col)
            .map(|(i, _)| next_start + i)
            .unwrap_or(next_end);
        true
    }

    /// The whitespace-delimited token ending at the caret (for @ / completion).
    pub fn token_at_cursor(&self) -> (usize, &str) {
        let before = &self.text[..self.cursor];
        let start = before
            .rfind(|c: char| c.is_whitespace())
            .map(|i| i + 1)
            .unwrap_or(0);
        (start, &self.text[start..self.cursor])
    }

    pub fn replace_range(&mut self, start: usize, end: usize, with: &str) {
        self.text.replace_range(start..end, with);
        self.cursor = start + with.len();
    }

    /// Soft-wrap into rows of at most `width` cells. Returns the rows and
    /// the caret's (row, col).
    pub fn layout(&self, width: usize, masked: bool) -> (Vec<String>, (usize, usize)) {
        let width = width.max(4);
        let mut rows: Vec<String> = vec![String::new()];
        let mut w = 0;
        let mut caret = (0, 0);
        for (i, c) in self.text.char_indices() {
            if i == self.cursor {
                caret = (rows.len() - 1, w);
            }
            if c == '\n' {
                rows.push(String::new());
                w = 0;
                continue;
            }
            let shown = if masked {
                '•'
            } else if c == '\t' {
                ' '
            } else {
                c
            };
            let cw = char_w(shown).max(if c == '\t' { 1 } else { 0 });
            if w + cw > width {
                rows.push(String::new());
                w = 0;
                if i == self.cursor {
                    caret = (rows.len() - 1, 0);
                }
            }
            rows.last_mut().unwrap().push(shown);
            w += cw;
        }
        if self.cursor >= self.text.len() {
            if w >= width {
                rows.push(String::new());
                w = 0;
            }
            caret = (rows.len() - 1, w);
        }
        (rows, caret)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editing_and_words() {
        let mut e = Editor::default();
        e.insert_str("hello big world");
        e.word_left();
        assert_eq!(&e.text[e.cursor..], "world");
        e.delete_word_back();
        assert_eq!(e.text, "hello world");
        e.home();
        e.kill_to_end();
        assert_eq!(e.text, "");
        e.insert_str("héllo");
        e.left();
        e.backspace();
        assert_eq!(e.text, "hélo");
    }

    #[test]
    fn multiline_navigation_and_layout() {
        let mut e = Editor::default();
        e.insert_str("abc\nde\nfghij");
        assert!(e.up());
        assert_eq!(&e.text[..e.cursor], "abc\nde");
        assert!(e.up());
        assert_eq!(&e.text[..e.cursor], "ab");
        assert!(!e.up());
        e.set("abcdefghij");
        let (rows, caret) = e.layout(4, false);
        assert_eq!(rows, vec!["abcd", "efgh", "ij"]);
        assert_eq!(caret, (2, 2));
        e.set("abcd");
        let (rows, caret) = e.layout(4, false);
        assert_eq!(rows, vec!["abcd", ""]);
        assert_eq!(caret, (1, 0));
        let (rows, _) = e.layout(10, true);
        assert_eq!(rows[0], "••••");
    }

    #[test]
    fn token_under_cursor() {
        let mut e = Editor::default();
        e.insert_str("look at @src/ma");
        let (start, tok) = e.token_at_cursor();
        assert_eq!(tok, "@src/ma");
        e.replace_range(start, e.cursor, "@src/main.rs ");
        assert_eq!(e.text, "look at @src/main.rs ");
    }
}
