//! Styled lines, wrapping, markdown and the Claude-Code-style views of
//! messages and tool calls. Pure functions: data in, lines out — so the
//! whole transcript can be re-rendered on resize or in verbose mode.

use crate::engine::PermDetail;
use crate::theme::Theme;
use crate::tools::{DiffLine, Display, TodoItem, ToolOutput};
use crossterm::style::Color;
use serde_json::Value;
use std::path::Path;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Style {
    pub fg: Option<Color>,
    pub bg: Option<Color>,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
}

impl Style {
    pub fn fg(c: Color) -> Self {
        Style {
            fg: Some(c),
            ..Default::default()
        }
    }
    pub fn bold(mut self) -> Self {
        self.bold = true;
        self
    }
    pub fn italic(mut self) -> Self {
        self.italic = true;
        self
    }
    pub fn on(mut self, c: Color) -> Self {
        self.bg = Some(c);
        self
    }
    pub fn underlined(mut self) -> Self {
        self.underline = true;
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub style: Style,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Line {
    pub spans: Vec<Span>,
    /// Background filling the rest of the row (diff lines).
    pub fill: Option<Color>,
}

impl Line {
    pub fn new() -> Self {
        Line::default()
    }
    pub fn raw(t: impl Into<String>) -> Self {
        Line {
            spans: vec![Span {
                text: t.into(),
                style: Style::default(),
            }],
            fill: None,
        }
    }
    pub fn styled(t: impl Into<String>, s: Style) -> Self {
        Line {
            spans: vec![Span {
                text: t.into(),
                style: s,
            }],
            fill: None,
        }
    }
    pub fn push(mut self, t: impl Into<String>, s: Style) -> Self {
        self.spans.push(Span {
            text: t.into(),
            style: s,
        });
        self
    }
    pub fn add(&mut self, t: impl Into<String>, s: Style) {
        self.spans.push(Span {
            text: t.into(),
            style: s,
        });
    }
    pub fn width(&self) -> usize {
        self.spans
            .iter()
            .map(|s| UnicodeWidthStr::width(s.text.as_str()))
            .sum()
    }
    pub fn text(&self) -> String {
        self.spans.iter().map(|s| s.text.as_str()).collect()
    }
}

pub fn char_w(c: char) -> usize {
    if c == '\t' {
        4
    } else {
        UnicodeWidthChar::width(c).unwrap_or(0)
    }
}

pub fn str_w(s: &str) -> usize {
    s.chars().map(char_w).sum()
}

/// Truncate to `max` display cells, adding "…" when cut.
pub fn truncate(s: &str, max: usize) -> String {
    if str_w(s) <= max {
        return s.to_string();
    }
    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = char_w(c);
        if w + cw + 1 > max {
            break;
        }
        out.push(c);
        w += cw;
    }
    out.push('…');
    out
}

/// Word-wrap a line to `width` cells. Continuation rows are indented by
/// `indent` cells. Tabs become 4 spaces.
pub fn wrap(line: &Line, width: usize, indent: usize) -> Vec<Line> {
    let width = width.max(8);
    if line.width() <= width && !line.spans.iter().any(|s| s.text.contains('\t')) {
        return vec![line.clone()];
    }
    // Tokenise into (char, style) cells, then break at spaces.
    let cells: Vec<(char, Style)> = line
        .spans
        .iter()
        .flat_map(|s| {
            s.text.chars().flat_map(move |c| {
                if c == '\t' {
                    vec![(' ', s.style); 4]
                } else {
                    vec![(c, s.style)]
                }
            })
        })
        .collect();
    let mut rows: Vec<Vec<(char, Style)>> = Vec::new();
    let mut i = 0;
    let mut first = true;
    while i < cells.len() {
        let avail = if first {
            width
        } else {
            width.saturating_sub(indent).max(4)
        };
        let mut w = 0;
        let mut j = i;
        let mut last_space: Option<usize> = None;
        while j < cells.len() {
            let cw = char_w(cells[j].0);
            if w + cw > avail {
                break;
            }
            if cells[j].0 == ' ' {
                last_space = Some(j);
            }
            w += cw;
            j += 1;
        }
        let end = if j >= cells.len() {
            j
        } else if let Some(sp) = last_space.filter(|&sp| sp > i) {
            sp + 1
        } else {
            j.max(i + 1)
        };
        let mut row: Vec<(char, Style)> = Vec::new();
        if !first {
            row.extend(std::iter::repeat_n(
                (' ', Style::default()),
                indent.min(width.saturating_sub(4)),
            ));
        }
        let mut seg: Vec<(char, Style)> = cells[i..end].to_vec();
        // Drop trailing spaces at a soft break.
        if end < cells.len() {
            while seg.last().map(|c| c.0 == ' ').unwrap_or(false) {
                seg.pop();
            }
        }
        row.extend(seg);
        rows.push(row);
        i = end;
        // Skip leading spaces on the next row.
        while i < cells.len() && cells[i].0 == ' ' && end < cells.len() && last_space.is_some() {
            i += 1;
        }
        first = false;
    }
    rows.into_iter()
        .map(|r| {
            let mut l = Line {
                spans: Vec::new(),
                fill: line.fill,
            };
            for (c, st) in r {
                match l.spans.last_mut() {
                    Some(s) if s.style == st => s.text.push(c),
                    _ => l.spans.push(Span {
                        text: c.to_string(),
                        style: st,
                    }),
                }
            }
            l
        })
        .collect()
}

// ─────────────────────────── markdown ───────────────────────────

fn inline(text: &str, base: Style, t: &Theme) -> Vec<Span> {
    let mut out: Vec<Span> = Vec::new();
    let push = |out: &mut Vec<Span>, s: String, st: Style| {
        if s.is_empty() {
            return;
        }
        match out.last_mut() {
            Some(l) if l.style == st => l.text.push_str(&s),
            _ => out.push(Span { text: s, style: st }),
        }
    };
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    let mut buf = String::new();
    while i < chars.len() {
        let c = chars[i];
        // `code`
        if c == '`' {
            if let Some(end) = chars[i + 1..].iter().position(|&x| x == '`') {
                push(&mut out, std::mem::take(&mut buf), base);
                let code: String = chars[i + 1..i + 1 + end].iter().collect();
                push(
                    &mut out,
                    code,
                    Style {
                        fg: Some(t.code),
                        ..base
                    },
                );
                i += end + 2;
                continue;
            }
        }
        // **bold** / __bold__
        if (c == '*' || c == '_') && chars.get(i + 1) == Some(&c) {
            let pat = [c, c];
            if let Some(end) = (i + 2..chars.len().saturating_sub(1))
                .find(|&k| chars[k] == pat[0] && chars[k + 1] == pat[1])
            {
                push(&mut out, std::mem::take(&mut buf), base);
                let inner: String = chars[i + 2..end].iter().collect();
                for s in inline(&inner, base.bold(), t) {
                    push(&mut out, s.text, s.style);
                }
                i = end + 2;
                continue;
            }
        }
        // *italic* (not a list bullet, not spaced)
        if (c == '*' || c == '_')
            && chars
                .get(i + 1)
                .map(|n| !n.is_whitespace())
                .unwrap_or(false)
            && (i == 0 || !chars[i - 1].is_alphanumeric())
        {
            if let Some(end) = (i + 1..chars.len()).find(|&k| {
                chars[k] == c
                    && !chars[k - 1].is_whitespace()
                    && chars
                        .get(k + 1)
                        .map(|n| !n.is_alphanumeric())
                        .unwrap_or(true)
            }) {
                if end > i + 1 {
                    push(&mut out, std::mem::take(&mut buf), base);
                    let inner: String = chars[i + 1..end].iter().collect();
                    push(&mut out, inner, base.italic());
                    i = end + 1;
                    continue;
                }
            }
        }
        // ~~strike~~
        if c == '~' && chars.get(i + 1) == Some(&'~') {
            if let Some(end) = (i + 2..chars.len().saturating_sub(1))
                .find(|&k| chars[k] == '~' && chars[k + 1] == '~')
            {
                push(&mut out, std::mem::take(&mut buf), base);
                let inner: String = chars[i + 2..end].iter().collect();
                push(
                    &mut out,
                    inner,
                    Style {
                        strike: true,
                        ..base
                    },
                );
                i = end + 2;
                continue;
            }
        }
        // [text](url)
        if c == '[' {
            if let Some(close) = chars[i..].iter().position(|&x| x == ']').map(|p| p + i) {
                if chars.get(close + 1) == Some(&'(') {
                    if let Some(pend) = chars[close + 1..]
                        .iter()
                        .position(|&x| x == ')')
                        .map(|p| p + close + 1)
                    {
                        push(&mut out, std::mem::take(&mut buf), base);
                        let label: String = chars[i + 1..close].iter().collect();
                        let url: String = chars[close + 2..pend].iter().collect();
                        push(
                            &mut out,
                            label.clone(),
                            Style {
                                fg: Some(t.accent),
                                ..base
                            }
                            .underlined(),
                        );
                        if url != label {
                            push(
                                &mut out,
                                format!(" ({url})"),
                                Style {
                                    fg: Some(t.dim),
                                    ..base
                                },
                            );
                        }
                        i = pend + 1;
                        continue;
                    }
                }
            }
        }
        buf.push(c);
        i += 1;
    }
    push(&mut out, buf, base);
    out
}

fn is_table_sep(l: &str) -> bool {
    let t = l.trim();
    t.starts_with('|') && t.chars().all(|c| matches!(c, '|' | '-' | ':' | ' '))
}

fn table_cells(l: &str) -> Vec<String> {
    let t = l.trim().trim_start_matches('|').trim_end_matches('|');
    t.split('|').map(|c| c.trim().to_string()).collect()
}

fn render_table(rows: &[Vec<String>], width: usize, t: &Theme, out: &mut Vec<Line>) {
    let ncol = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    if ncol == 0 {
        return;
    }
    let mut w: Vec<usize> = vec![1; ncol];
    for r in rows {
        for (i, c) in r.iter().enumerate() {
            w[i] = w[i].max(str_w(&strip_md(c)));
        }
    }
    // Shrink the widest columns until the table fits.
    let budget = width.saturating_sub(ncol * 3 + 1);
    while w.iter().sum::<usize>() > budget && w.iter().any(|&x| x > 6) {
        let (mi, _) = w.iter().enumerate().max_by_key(|(_, &x)| x).unwrap();
        w[mi] -= 1;
    }
    let border = Style::fg(t.dim);
    let hline = |l: &str, m: &str, r: &str| {
        let mut s = String::from(l);
        for (i, cw) in w.iter().enumerate() {
            s.push_str(&"─".repeat(cw + 2));
            s.push_str(if i + 1 < ncol { m } else { r });
        }
        Line::styled(s, border)
    };
    out.push(hline("┌", "┬", "┐"));
    for (ri, r) in rows.iter().enumerate() {
        let mut l = Line::styled("│", border);
        for (i, cw) in w.iter().enumerate() {
            let cell = r.get(i).map(|c| strip_md(c)).unwrap_or_default();
            let cell = truncate(&cell, *cw);
            let pad = cw - str_w(&cell);
            let st = if ri == 0 {
                Style::default().bold()
            } else {
                Style::default()
            };
            l.add(format!(" {cell}{} ", " ".repeat(pad)), st);
            l.add("│", border);
        }
        out.push(l);
        if ri == 0 && rows.len() > 1 {
            out.push(hline("├", "┼", "┤"));
        }
    }
    out.push(hline("└", "┴", "┘"));
}

fn strip_md(s: &str) -> String {
    s.replace("**", "").replace('`', "")
}

/// Render markdown into wrapped lines. `indent` prefixes every row.
pub fn markdown(src: &str, width: usize, indent: &str, t: &Theme) -> Vec<Line> {
    let mut out = Vec::new();
    let iw = str_w(indent);
    let inner = width.saturating_sub(iw).max(10);
    let lines: Vec<&str> = src.lines().collect();
    let mut i = 0;
    let mut in_code = false;
    let mut raw: Vec<Line> = Vec::new();
    while i < lines.len() {
        let line = lines[i].trim_end();
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_code = !in_code;
            i += 1;
            continue;
        }
        if in_code {
            raw.push(Line::styled(
                format!("  {}", line.replace('\t', "    ")),
                Style::fg(t.code_block),
            ));
            i += 1;
            continue;
        }
        // Tables.
        if trimmed.starts_with('|') && i + 1 < lines.len() && is_table_sep(lines[i + 1]) {
            let mut rows = vec![table_cells(trimmed)];
            i += 2;
            while i < lines.len() && lines[i].trim_start().starts_with('|') {
                rows.push(table_cells(lines[i]));
                i += 1;
            }
            let mut tl = Vec::new();
            render_table(&rows, inner, t, &mut tl);
            raw.extend(tl);
            continue;
        }
        if trimmed.is_empty() {
            raw.push(Line::new());
            i += 1;
            continue;
        }
        if trimmed.chars().all(|c| c == '-' || c == '*' || c == '_') && trimmed.len() >= 3 {
            raw.push(Line::styled("─".repeat(inner.min(60)), Style::fg(t.dim)));
            i += 1;
            continue;
        }
        let hashes = trimmed.chars().take_while(|&c| c == '#').count();
        if (1..=6).contains(&hashes) && trimmed[hashes..].starts_with(' ') {
            let text = trimmed[hashes + 1..].trim();
            let st = if hashes <= 2 {
                Style::default().bold().underlined()
            } else {
                Style::default().bold()
            };
            let mut l = Line::new();
            l.spans = inline(text, st, t);
            raw.push(l);
            i += 1;
            continue;
        }
        if let Some(q) = trimmed.strip_prefix('>') {
            let mut l = Line::styled("▎ ", Style::fg(t.dim));
            l.spans
                .extend(inline(q.trim_start(), Style::fg(t.dim).italic(), t));
            raw.push(l);
            i += 1;
            continue;
        }
        let lead = line.len() - trimmed.len();
        let bullet = ["- ", "* ", "+ "].iter().find(|b| trimmed.starts_with(**b));
        if let Some(b) = bullet {
            let depth = lead / 2;
            let glyph = if depth.is_multiple_of(2) {
                "• "
            } else {
                "◦ "
            };
            let mut rest = &trimmed[b.len()..];
            let mut prefix = format!("{}{glyph}", "  ".repeat(depth));
            if let Some(r) = rest.strip_prefix("[ ] ") {
                prefix = format!("{}☐ ", "  ".repeat(depth));
                rest = r;
            } else if let Some(r) = rest
                .strip_prefix("[x] ")
                .or_else(|| rest.strip_prefix("[X] "))
            {
                prefix = format!("{}☒ ", "  ".repeat(depth));
                rest = r;
            }
            let mut l = Line::raw(prefix.clone());
            l.spans.extend(inline(rest, Style::default(), t));
            for w in wrap(&l, inner, str_w(&prefix)) {
                raw.push(w);
            }
            i += 1;
            continue;
        }
        let num = trimmed.chars().take_while(|c| c.is_ascii_digit()).count();
        if num > 0 && (trimmed[num..].starts_with(". ") || trimmed[num..].starts_with(") ")) {
            let prefix = format!("{}{} ", "  ".repeat(lead / 2), &trimmed[..num + 1]);
            let mut l = Line::raw(prefix.clone());
            l.spans
                .extend(inline(&trimmed[num + 2..], Style::default(), t));
            for w in wrap(&l, inner, str_w(&prefix)) {
                raw.push(w);
            }
            i += 1;
            continue;
        }
        let mut l = Line::new();
        l.spans = inline(line, Style::default(), t);
        for w in wrap(&l, inner, 0) {
            raw.push(w);
        }
        i += 1;
    }
    // Collapse runs of blank lines and trim the ends.
    let mut prev_blank = true;
    for l in raw {
        let blank = l.width() == 0;
        if blank && prev_blank {
            continue;
        }
        prev_blank = blank;
        let mut row = Line::raw(indent.to_string());
        row.spans.extend(l.spans);
        row.fill = l.fill;
        out.extend(wrap(&row, width, iw));
    }
    while out.last().map(|l| l.width() <= iw).unwrap_or(false) {
        out.pop();
    }
    out
}

// ─────────────────────────── tool call views ───────────────────────────

pub fn compact_json(v: &Value, max: usize) -> String {
    let s = match v {
        Value::Object(o) => o
            .iter()
            .map(|(k, v)| {
                format!(
                    "{k}: {}",
                    match v {
                        Value::String(s) => format!("\"{}\"", s.replace('\n', "\\n")),
                        other => other.to_string(),
                    }
                )
            })
            .collect::<Vec<_>>()
            .join(", "),
        other => other.to_string(),
    };
    truncate(&s, max)
}

fn rel(p: &str, root: &Path) -> String {
    let pb = crate::tools::normalize(&root.join(p));
    let r = crate::tools::normalize(root);
    match pb.strip_prefix(&r) {
        Ok(x) if !x.as_os_str().is_empty() => x.display().to_string(),
        _ => p.to_string(),
    }
}

/// "Read(src/main.rs)", "Bash(cargo test)", "Update(src/lib.rs)", …
pub fn tool_call_title(name: &str, input: &Value, root: &Path) -> String {
    let s = |k: &str| {
        input
            .get(k)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };
    let (label, arg) = match name {
        "Read" => {
            let mut a = rel(&s("file_path"), root);
            if let (Some(o), Some(l)) = (input.get("offset"), input.get("limit")) {
                a = format!(
                    "{a}, lines {}-{}",
                    o,
                    o.as_u64().unwrap_or(1) + l.as_u64().unwrap_or(0)
                );
            }
            ("Read".to_string(), a)
        }
        "Write" => ("Write".into(), rel(&s("file_path"), root)),
        "Edit" | "MultiEdit" => ("Update".into(), rel(&s("file_path"), root)),
        "Bash" => (
            "Bash".into(),
            s("command").lines().next().unwrap_or("").to_string()
                + if s("command").lines().count() > 1 {
                    " …"
                } else {
                    ""
                },
        ),
        "BashOutput" => ("BashOutput".into(), s("bash_id")),
        "KillShell" => ("Kill Shell".into(), s("shell_id")),
        "Glob" => {
            let mut a = format!("pattern: \"{}\"", s("pattern"));
            if !s("path").is_empty() {
                a.push_str(&format!(", path: \"{}\"", rel(&s("path"), root)));
            }
            ("Search".into(), a)
        }
        "Grep" => {
            let mut a = format!("pattern: \"{}\"", s("pattern"));
            if !s("path").is_empty() {
                a.push_str(&format!(", path: \"{}\"", rel(&s("path"), root)));
            }
            for k in ["glob", "type", "output_mode"] {
                if !s(k).is_empty() {
                    a.push_str(&format!(", {k}: \"{}\"", s(k)));
                }
            }
            ("Search".into(), a)
        }
        "LS" => ("List".into(), rel(&s("path"), root)),
        "WebFetch" => ("Fetch".into(), s("url")),
        "WebSearch" => ("Web Search".into(), format!("\"{}\"", s("query"))),
        "TodoWrite" => ("Update Todos".into(), String::new()),
        "Task" => (
            if s("subagent_type").is_empty() || s("subagent_type") == "general-purpose" {
                "Task".to_string()
            } else {
                s("subagent_type")
            },
            s("description"),
        ),
        "ExitPlanMode" => ("Plan".into(), String::new()),
        "Skill" => ("Skill".into(), s("skill")),
        n if n.starts_with("mcp__") => {
            let mut parts = n.splitn(3, "__").skip(1);
            let server = parts.next().unwrap_or("");
            let tool = parts.next().unwrap_or("");
            (format!("{server} - {tool} (MCP)"), compact_json(input, 80))
        }
        other => (other.to_string(), compact_json(input, 80)),
    };
    if arg.is_empty() {
        label
    } else {
        format!("{label}({arg})")
    }
}

pub fn human_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}m", n as f64 / 1e6)
    } else if n >= 1000 {
        format!("{:.1}k", n as f64 / 1e3)
    } else {
        n.to_string()
    }
}

const RESULT_PREFIX: &str = "  ⎿  ";
const RESULT_INDENT: &str = "     ";

/// Header line "⏺ Read(src/main.rs)" with the dot's state colour.
pub fn tool_header(title: &str, dot: Style, width: usize) -> Vec<Line> {
    let (name, args) = match title.find('(') {
        Some(i) => (&title[..i], &title[i..]),
        None => (title, ""),
    };
    let l = Line::styled("⏺ ", dot)
        .push(name, Style::default().bold())
        .push(args, Style::default());
    wrap(&l, width, 2)
}

pub fn diff_view(
    lines: &[DiffLine],
    width: usize,
    t: &Theme,
    max_lines: Option<usize>,
) -> Vec<Line> {
    let numw = lines
        .iter()
        .map(|l| l.lineno)
        .max()
        .unwrap_or(1)
        .to_string()
        .len()
        .max(3);
    let mut out = Vec::new();
    let shown = max_lines.map(|m| m.min(lines.len())).unwrap_or(lines.len());
    for d in &lines[..shown] {
        if d.kind == '…' {
            out.push(
                Line::styled(
                    format!("{RESULT_INDENT}{}", " ".repeat(numw)),
                    Style::fg(t.dim),
                )
                .push(" ...", Style::fg(t.dim)),
            );
            continue;
        }
        let (bg, sign) = match d.kind {
            '+' => (Some(t.add_bg), "+"),
            '-' => (Some(t.del_bg), "-"),
            _ => (None, " "),
        };
        let st = Style {
            bg,
            ..Default::default()
        };
        let num = Style {
            fg: Some(t.dim),
            bg,
            ..Default::default()
        };
        let text = d.text.replace('\t', "    ");
        let l = Line {
            spans: vec![],
            fill: None,
        }
        .push(RESULT_INDENT, Style::default())
        .push(format!("{:>numw$} ", d.lineno), num)
        .push(format!("{sign} "), st)
        .push(text, st);
        let mut rows = wrap(&l, width, RESULT_INDENT.len() + numw + 3);
        for r in rows.iter_mut() {
            r.fill = bg;
        }
        out.extend(rows);
    }
    if shown < lines.len() {
        out.push(Line::styled(
            format!(
                "{RESULT_INDENT}… +{} lines (ctrl+r to expand)",
                lines.len() - shown
            ),
            Style::fg(t.dim),
        ));
    }
    out
}

pub fn todo_lines(todos: &[TodoItem], t: &Theme, prefix_first: &str) -> Vec<Line> {
    let mut out = Vec::new();
    for (i, td) in todos.iter().enumerate() {
        let pre = if i == 0 { prefix_first } else { RESULT_INDENT };
        let (glyph, st) = match td.status.as_str() {
            "completed" => (
                "☒ ",
                Style {
                    strike: true,
                    fg: Some(t.dim),
                    ..Default::default()
                },
            ),
            "in_progress" => ("☐ ", Style::default().bold()),
            _ => ("☐ ", Style::default()),
        };
        let glyph_st = if td.status == "completed" {
            Style::fg(t.ok)
        } else {
            Style::default()
        };
        out.push(
            Line::styled(pre, Style::fg(t.dim))
                .push(glyph, glyph_st)
                .push(td.content.clone(), st),
        );
    }
    out
}

/// Result lines under a finished tool call.
pub fn tool_result(
    name: &str,
    out: &ToolOutput,
    width: usize,
    t: &Theme,
    verbose: bool,
) -> Vec<Line> {
    let dim = Style::fg(t.dim);
    let err = Style::fg(t.err);
    let mut v = Vec::new();
    match &out.display {
        Display::Todos(todos) => v.extend(todo_lines(todos, t, RESULT_PREFIX)),
        Display::Diff { summary, lines } => {
            v.push(Line::styled(RESULT_PREFIX, dim).push(summary.clone(), Style::default()));
            let cap = if verbose {
                None
            } else if name == "Write" {
                Some(10)
            } else {
                Some(40)
            };
            v.extend(diff_view(lines, width, t, cap));
        }
        Display::Lines { summary, body } => {
            let style = if out.is_error { err } else { Style::default() };
            let rejected = summary.starts_with("User rejected");
            if rejected {
                v.push(Line::styled(RESULT_PREFIX, dim).push(summary.clone(), err));
                return v;
            }
            let body: Vec<&String> = body.iter().collect();
            let limit = if verbose {
                usize::MAX
            } else if out.is_error {
                10
            } else {
                4
            };
            let mut first = true;
            if !summary.is_empty() {
                let mut l = Line::styled(RESULT_PREFIX, dim).push(summary.clone(), style);
                if !verbose && !body.is_empty() && matches!(name, "Read" | "Glob" | "Grep" | "LS") {
                    l.add(" (ctrl+r to expand)", dim);
                }
                v.extend(wrap(&l, width, RESULT_PREFIX.len()));
                first = false;
            }
            let show_body = verbose
                || !matches!(
                    name,
                    "Read" | "Glob" | "Grep" | "LS" | "WebFetch" | "WebSearch" | "Task"
                );
            if show_body {
                let n = body.len();
                for b in body.iter().take(limit) {
                    let pre = if first { RESULT_PREFIX } else { RESULT_INDENT };
                    first = false;
                    let l = Line::styled(pre, dim).push(
                        b.replace('\t', "    "),
                        if out.is_error { err } else { dim },
                    );
                    v.extend(wrap(&l, width, RESULT_PREFIX.len()));
                }
                if n > limit {
                    v.push(Line::styled(
                        format!("{RESULT_INDENT}… +{} lines (ctrl+r to expand)", n - limit),
                        dim,
                    ));
                }
            }
            if first {
                v.push(Line::styled(RESULT_PREFIX, dim).push(
                    if out.is_error {
                        "Error"
                    } else {
                        "(No content)"
                    },
                    if out.is_error { err } else { dim },
                ));
            }
        }
    }
    v
}

// ─────────────────────────── permission prompt body ───────────────────────────

pub fn permission_body(
    tool: &str,
    detail: &PermDetail,
    width: usize,
    t: &Theme,
) -> (String, Vec<Line>, String) {
    match detail {
        PermDetail::Command {
            command,
            description,
        } => {
            let mut v = Vec::new();
            for l in command.lines() {
                v.extend(wrap(&Line::raw(format!("  {l}")), width, 2));
            }
            if !description.is_empty() {
                v.push(Line::styled(format!("  {description}"), Style::fg(t.dim)));
            }
            ("Bash command".into(), v, "Do you want to proceed?".into())
        }
        PermDetail::Diff { title, lines } => {
            let file = title
                .split_once(' ')
                .map(|x| x.1)
                .unwrap_or(title)
                .to_string();
            let fname = Path::new(&file)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(&file)
                .to_string();
            let create = title.starts_with("Create");
            let mut v = diff_view(lines, width, t, Some(24));
            if v.is_empty() {
                v.push(Line::styled("  (no preview available)", Style::fg(t.dim)));
            }
            (
                if create {
                    "Create file".into()
                } else {
                    "Edit file".into()
                },
                v,
                if create {
                    format!("Do you want to create {fname}?")
                } else {
                    format!("Do you want to make this edit to {fname}?")
                },
            )
        }
        PermDetail::Plan(plan) => {
            let mut v = vec![Line::raw("Here is ICE's plan:"), Line::new()];
            v.extend(markdown(plan, width, "  ", t));
            (
                "Ready to code?".into(),
                v,
                "Would you like to proceed?".into(),
            )
        }
        PermDetail::Fetch { url } => (
            "Fetch".into(),
            vec![Line::raw(format!("  {url}"))],
            "Do you want to allow ICE to fetch this content?".to_string(),
        ),
        PermDetail::Generic { summary } => {
            let title = if tool.starts_with("mcp__") {
                "Tool use".to_string()
            } else {
                tool.to_string()
            };
            (
                title,
                wrap(&Line::raw(format!("  {summary}")), width, 2),
                "Do you want to proceed?".into(),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn th() -> Theme {
        crate::theme::ThemeName::DEFAULT.palette()
    }

    #[test]
    fn wrap_breaks_on_words_and_indents() {
        let l = Line::raw("alpha beta gamma delta epsilon");
        let rows = wrap(&l, 12, 2);
        assert!(rows.iter().all(|r| r.width() <= 12), "{rows:?}");
        assert_eq!(rows[0].text(), "alpha beta");
        assert!(rows[1].text().starts_with("  "));
        // Wide characters count as two cells.
        let rows = wrap(&Line::raw("日本語テキストです"), 8, 0);
        assert!(rows.iter().all(|r| r.width() <= 8));
    }

    #[test]
    fn markdown_renders_blocks() {
        let t = th();
        let md = "# Title\n\nSome **bold** and `code` and [a link](https://x.y).\n\n- one\n  - two\n1. first\n\n```rust\nfn main() {}\n```\n\n| a | b |\n|---|---|\n| 1 | 2 |";
        let lines = markdown(md, 60, "", &t);
        let text: Vec<String> = lines.iter().map(|l| l.text()).collect();
        assert_eq!(text[0], "Title");
        assert!(
            text.iter()
                .any(|l| l.contains("Some bold and code and a link (https://x.y).")),
            "{text:?}"
        );
        assert!(text.iter().any(|l| l == "• one"));
        assert!(text.iter().any(|l| l == "  ◦ two"));
        assert!(text.iter().any(|l| l == "1. first"));
        assert!(text.iter().any(|l| l == "  fn main() {}"));
        assert!(text.iter().any(|l| l.starts_with("┌")));
        assert!(text.iter().any(|l| l.contains("│ 1 │ 2 │")), "{text:?}");
    }

    #[test]
    fn tool_titles_match_claude_code() {
        let root = Path::new("/p");
        assert_eq!(
            tool_call_title(
                "Read",
                &serde_json::json!({"file_path":"/p/src/a.rs"}),
                root
            ),
            "Read(src/a.rs)"
        );
        assert_eq!(
            tool_call_title("Edit", &serde_json::json!({"file_path":"src/a.rs"}), root),
            "Update(src/a.rs)"
        );
        assert_eq!(
            tool_call_title("Bash", &serde_json::json!({"command":"cargo test"}), root),
            "Bash(cargo test)"
        );
        assert_eq!(
            tool_call_title("Grep", &serde_json::json!({"pattern":"fn"}), root),
            "Search(pattern: \"fn\")"
        );
        assert_eq!(
            tool_call_title("TodoWrite", &serde_json::json!({}), root),
            "Update Todos"
        );
        assert_eq!(
            tool_call_title("mcp__gh__issue", &serde_json::json!({"n":1}), root),
            "gh - issue (MCP)(n: 1)"
        );
        assert_eq!(human_tokens(1234), "1.2k");
    }
}
