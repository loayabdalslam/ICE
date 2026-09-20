use crate::app::{App, MsgKind, Screen};
use crate::providers::{self, PROVIDERS};
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Gauge, List, ListItem, Paragraph, Wrap};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

pub fn draw(f: &mut Frame, app: &App) {
    let theme = app.theme;
    if f.size().width < 36 || f.size().height < 14 {
        f.render_widget(
            Paragraph::new(
                "ICE / expand your terminal
Minimum: 36 columns × 14 rows
Ctrl+C to exit",
            )
            .style(Style::default().fg(theme.ice).bg(theme.bg)),
            f.size(),
        );
        return;
    }
    f.render_widget(
        Block::default().style(Style::default().bg(theme.bg).fg(theme.text)),
        f.size(),
    );

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(3),
            Constraint::Length(4),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(f.size());

    draw_top(f, app, chunks[0]);
    match app.screen {
        Screen::Onboard => draw_onboard(f, app, chunks[1]),
        Screen::Welcome => draw_welcome(f, app, chunks[1]),
        Screen::Session => draw_session(f, app, chunks[1]),
    }
    draw_input(f, app, chunks[2]);
    draw_context(f, app, chunks[3]);
    draw_status(f, app, chunks[4]);
    if app.help {
        draw_help(f, app);
    }
}

fn draw_top(f: &mut Frame, app: &App, area: Rect) {
    let t = app.theme;
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(25), Constraint::Min(0)])
        .split(inset(area, 1, 0));
    let block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(Style::default().fg(t.border));
    f.render_widget(block, area);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" ■ ICE ", t.title()),
            Span::styled(format!(" / {}", env!("CARGO_PKG_VERSION")), t.dim()),
        ])),
        Rect::new(cols[0].x, cols[0].y + 1, cols[0].width, 1),
    );
    let (label, color) = if let Some(note) = &app.update_note {
        (
            format!("⬆ {note}  "),
            if note.starts_with('✓') { t.ok } else { t.accent },
        )
    } else {
        (
            format!(
                "{}  ·  {}  ",
                short_path(&app.root),
                if app.demo { "DEMO" } else { "LIVE" }
            ),
            t.muted,
        )
    };
    f.render_widget(
        Paragraph::new(label)
            .style(Style::default().fg(color))
            .alignment(Alignment::Right),
        Rect::new(cols[1].x, cols[1].y + 1, cols[1].width, 1),
    );
}

fn draw_onboard(f: &mut Frame, app: &App, area: Rect) {
    let t = app.theme;
    let area = if area.width >= 100 && area.height >= 17 {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(43), Constraint::Min(40)])
            .split(inset(area, 1, 1));
        crate::mascot::draw(f, cols[0], t, app.tick, false);
        f.render_widget(
            Paragraph::new("MEET FLOE. YOUR ICE COMPANION.\nLet's make something worth shipping.")
                .style(t.dim())
                .alignment(Alignment::Center),
            Rect::new(cols[0].x, cols[0].y + 14, cols[0].width, 2),
        );
        cols[1]
    } else {
        inset(area, 1, 0)
    };
    let title = match app.onboard_step {
        0 => " CONNECT / 01 PROVIDER → 02 KEY → 03 MODEL ",
        1 => " CONNECT / 01 provider → 02 KEY → 03 model ",
        2 => " CONNECT / 01 provider → 02 key → 03 MODEL ",
        _ => " onboard ",
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(t.border_focus))
        .title(Span::styled(title, Style::default().fg(t.accent)));
    let inner = block.inner(area);
    f.render_widget(block, area);

    match app.onboard_step {
        0 => {
            // Combined list: API providers, then CLI-agent backends.
            let np = PROVIDERS.len();
            let mut rows: Vec<(bool, String)> = Vec::new();
            for p in PROVIDERS {
                let ready = providers::key_for(p).is_some();
                rows.push((ready, format!("{:<12}  {}", p.id, p.default_model)));
            }
            for (ci, c) in providers::CLI_AGENTS.iter().enumerate() {
                let ready = app.cli_present.get(ci).copied().unwrap_or(false);
                rows.push((ready, format!("cli:{:<8}  {}", c.id, c.name)));
            }
            let items: Vec<ListItem> = rows
                .into_iter()
                .enumerate()
                .skip(
                    app.onboard_idx
                        .saturating_sub(inner.height.saturating_sub(1) as usize),
                )
                .take(inner.height as usize)
                .map(|(i, (ready, label))| {
                    let mark = if ready { "●" } else { "○" };
                    let sel = if i == app.onboard_idx { "▸" } else { " " };
                    let tag = if i >= np { " ⌘" } else { "  " };
                    ListItem::new(format!(" {sel} {mark}{tag} {label}")).style(
                        if i == app.onboard_idx {
                            Style::default()
                                .fg(t.bg)
                                .bg(t.accent)
                                .add_modifier(Modifier::BOLD)
                        } else if i >= np {
                            Style::default().fg(t.ice)
                        } else {
                            t.body()
                        },
                    )
                })
                .collect();
            f.render_widget(List::new(items).style(Style::default().fg(t.text)), inner);
        }
        1 => {
            let p = PROVIDERS[app.onboard_idx.min(PROVIDERS.len() - 1)];
            let have = providers::key_for(&p).is_some();
            let lines = vec![
                Line::from(format!("provider  {}", p.name)),
                Line::from(format!("env       {:?}", p.key_envs)),
                Line::from(if have {
                    "status    key already in environment — press enter"
                } else {
                    "status    paste a key in the input widget then enter"
                }),
                Line::from(""),
                Line::from(
                    "keys stay in the process env. they are not written to ~/.ice/config.json",
                ),
                Line::from("esc skips onboarding"),
            ];
            f.render_widget(
                Paragraph::new(lines).style(Style::default().fg(t.text)),
                inner,
            );
        }
        2 => {
            let items: Vec<ListItem> = app
                .models
                .iter()
                .enumerate()
                .skip(
                    app.model_idx
                        .saturating_sub(inner.height.saturating_sub(1) as usize),
                )
                .take(inner.height as usize)
                .map(|(i, m)| {
                    let sel = if i == app.model_idx { "▸" } else { " " };
                    ListItem::new(format!(" {sel} {m}")).style(if i == app.model_idx {
                        Style::default().fg(t.bg).bg(t.accent)
                    } else {
                        t.body()
                    })
                })
                .collect();
            if items.is_empty() {
                f.render_widget(
                    Paragraph::new("no models yet — enter uses the provider default"),
                    inner,
                );
            } else {
                f.render_widget(List::new(items).style(Style::default().fg(t.text)), inner);
            }
        }
        _ => {}
    }
}

fn draw_welcome(f: &mut Frame, app: &App, area: Rect) {
    let t = app.theme;
    let content = centered(area, 104, 24);
    let wide = content.width >= 80 && content.height >= 17;
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(5),
            Constraint::Length(if content.height >= 20 { 6 } else { 3 }),
        ])
        .split(content);
    let hero = if wide {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(49), Constraint::Percentage(51)])
            .split(rows[0]);
        crate::mascot::draw(f, inset(cols[0], 0, 1), t, app.tick, app.running);
        cols[1]
    } else {
        rows[0]
    };
    let mut lines = if hero.height >= 12 {
        crate::mascot::wordmark(t)
    } else {
        vec![Line::from(Span::styled("■ ICE", t.title()))]
    };
    lines.extend([
        Line::from(""),
        Line::from(Span::styled("INTENT. COMPILE. EXECUTE.", t.title())),
        Line::from(Span::styled(
            "A clear path from thought to shipped.",
            t.dim(),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                "● ",
                Style::default().fg(if app.demo { t.warn } else { t.ok }),
            ),
            Span::styled(
                if app.demo {
                    "Offline demo"
                } else {
                    "Connected"
                },
                t.body(),
            ),
            Span::styled(format!("  /  {}", app.provider), t.dim()),
        ]),
        Line::from(Span::styled(app.model.clone(), t.dim())),
    ]);
    f.render_widget(
        Paragraph::new(lines).alignment(if wide {
            Alignment::Left
        } else {
            Alignment::Center
        }),
        inset(hero, 2, 1),
    );
    if rows[1].height >= 6 {
        let cards = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(34),
                Constraint::Percentage(33),
                Constraint::Percentage(33),
            ])
            .split(rows[1]);
        for (i, (title, cmd, detail)) in [
            ("01 / CONNECT", "/onboard", "Choose your model"),
            ("02 / DIRECT", "/goal", "Give intent a direction"),
            ("03 / DISCOVER", "/help", "Explore your workspace"),
        ]
        .iter()
        .enumerate()
        {
            f.render_widget(
                Paragraph::new(vec![
                    Line::from(Span::styled(*cmd, t.title())),
                    Line::from(Span::styled(*detail, t.dim())),
                ])
                .block(
                    Block::default()
                        .title(*title)
                        .title_style(t.dim())
                        .borders(Borders::ALL)
                        .border_type(BorderType::Rounded)
                        .border_style(Style::default().fg(t.border)),
                ),
                inset(cards[i], 1, 0),
            );
        }
    } else {
        f.render_widget(
            Paragraph::new("/onboard   /goal   /help")
                .style(t.dim())
                .alignment(Alignment::Center),
            rows[1],
        );
    }
}

fn draw_session(f: &mut Frame, app: &App, area: Rect) {
    let t = app.theme;
    let area = if area.width >= 100 {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(50), Constraint::Length(30)])
            .split(area);
        draw_todos(f, app, cols[1]);
        cols[0]
    } else {
        area
    };
    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(t.border))
        .title(Span::styled(" CONVERSATION ", Style::default().fg(t.muted)));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines: Vec<Line> = Vec::new();
    for msg in &app.messages {
        render_msg(&mut lines, msg, &t);
    }
    if app.running {
        let spin = ["◐", "◓", "◑", "◒"][(app.tick / 2 % 4) as usize];
        lines.push(Line::from(Span::styled(
            format!("{spin} {}…", app.status),
            Style::default().fg(t.accent),
        )));
    }
    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "  no turns yet — type in the input widget",
            Style::default().fg(t.muted),
        )));
    }
    let visual_lines: usize = lines
        .iter()
        .map(|line| {
            (line.width().max(1) + inner.width.max(1) as usize - 1) / inner.width.max(1) as usize
        })
        .sum();
    let max_scroll = visual_lines
        .saturating_sub(inner.height as usize)
        .min(u16::MAX as usize) as u16;
    app.max_scroll.set(max_scroll);
    // Auto-scroll: stick to the newest output unless the user scrolled up.
    let scroll = if app.follow {
        max_scroll
    } else {
        app.scroll.min(max_scroll)
    };
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0)),
        inner,
    );
}

/// Map an ICE step title ("read foo.rs", "run ls -la") to Claude-style
/// (ToolName, args) — e.g. ("Read", "foo.rs"), ("Bash", "ls -la").
fn tool_display(title: &str) -> (String, String) {
    let (verb, rest) = match title.split_once(char::is_whitespace) {
        Some((v, r)) => (v, r.trim()),
        None => (title, ""),
    };
    let name = match verb {
        "read" => "Read",
        "write" => "Write",
        "replace" | "patch" => "Update",
        "list" | "ls" => "List",
        "grep" => "Grep",
        "run" | "exec" | "sh" | "bash" => "Bash",
        "web_search" | "websearch" => "Search",
        "web_fetch" | "webfetch" | "fetch" => "Fetch",
        "mcp" | "tool" => "MCP",
        "skill" => "Skill",
        "todo" => "Todo",
        "agent" => "Task",
        other => return (title_case(other), rest.to_string()),
    };
    (name.to_string(), rest.to_string())
}

fn title_case(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => s.to_string(),
    }
}

/// Build a Claude-style result summary + output preview from a step body.
/// Body is "exit <n> · <ms>ms" optionally followed by output lines.
fn tool_result(name: &str, body: &str, ok: bool) -> (String, Vec<String>) {
    let mut lines = body.lines();
    let head = lines.next().unwrap_or("");
    let output: Vec<String> = lines.map(|l| l.to_string()).collect();
    let n = output.len();
    let summary = match name {
        "Read" => format!("Read {n} line{}", plural(n)),
        "List" => format!("{n} entr{}", if n == 1 { "y" } else { "ies" }),
        "Grep" | "Search" => format!("{n} result{}", plural(n)),
        "Write" | "Update" => output
            .first()
            .cloned()
            .unwrap_or_else(|| if ok { "done".into() } else { head.to_string() }),
        "Bash" | "Fetch" | "MCP" => {
            if n == 0 {
                if ok { head.to_string() } else { format!("{head} (failed)") }
            } else {
                format!("{n} line{}", plural(n))
            }
        }
        _ => {
            if n == 0 {
                head.to_string()
            } else {
                output[0].clone()
            }
        }
    };
    // Preview: reads/lists stay collapsed; commands/searches show output.
    let preview = match name {
        "Read" | "List" | "Write" | "Update" => Vec::new(),
        _ => output,
    };
    (summary, preview)
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

/// Render a lightweight-markdown body into styled lines: fenced code blocks,
/// `#` headings, `-`/`*`/numbered bullets, `>` quotes, and inline `**bold**`
/// and `` `code` ``. Keeps ICE's theme; no external markdown dependency.
fn push_markdown(out: &mut Vec<Line<'static>>, body: &str, t: &crate::theme::Theme, indent: &str) {
    let text = Style::default().fg(t.text);
    let dim = Style::default().fg(t.muted);
    let code = Style::default().fg(t.accent).bg(t.surface);
    let mut in_code = false;
    for raw in body.lines() {
        let line = raw.trim_end();
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            in_code = !in_code;
            continue;
        }
        if in_code {
            out.push(Line::from(Span::styled(format!("{indent}{line}"), code)));
            continue;
        }
        if let Some(h) = trimmed.strip_prefix("### ").or_else(|| trimmed.strip_prefix("## ")).or_else(|| trimmed.strip_prefix("# ")) {
            out.push(Line::from(Span::styled(
                format!("{indent}{h}"),
                Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
            )));
            continue;
        }
        if trimmed.starts_with("> ") {
            out.push(Line::from(Span::styled(
                format!("{indent}▏{}", &trimmed[2..]),
                dim,
            )));
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")) {
            let mut spans = vec![Span::styled(format!("{indent}• "), Style::default().fg(t.accent))];
            spans.extend(md_inline(rest, text, t));
            out.push(Line::from(spans));
            continue;
        }
        let mut spans = vec![Span::styled(indent.to_string(), text)];
        spans.extend(md_inline(trimmed, text, t));
        out.push(Line::from(spans));
    }
}

/// Split a line into spans, styling `**bold**` and `` `code` `` runs.
fn md_inline(line: &str, base: Style, t: &crate::theme::Theme) -> Vec<Span<'static>> {
    let code = Style::default().fg(t.accent).bg(t.surface);
    let bold = base.add_modifier(Modifier::BOLD);
    let mut spans = Vec::new();
    let mut buf = String::new();
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '`' {
            if !buf.is_empty() {
                spans.push(Span::styled(std::mem::take(&mut buf), base));
            }
            let mut inner = String::new();
            while let Some(&n) = chars.peek() {
                chars.next();
                if n == '`' {
                    break;
                }
                inner.push(n);
            }
            spans.push(Span::styled(inner, code));
        } else if c == '*' && chars.peek() == Some(&'*') {
            chars.next();
            if !buf.is_empty() {
                spans.push(Span::styled(std::mem::take(&mut buf), base));
            }
            let mut inner = String::new();
            while let Some(&n) = chars.peek() {
                chars.next();
                if n == '*' && chars.peek() == Some(&'*') {
                    chars.next();
                    break;
                }
                inner.push(n);
            }
            spans.push(Span::styled(inner, bold));
        } else {
            buf.push(c);
        }
    }
    if !buf.is_empty() {
        spans.push(Span::styled(buf, base));
    }
    if spans.is_empty() {
        spans.push(Span::styled(String::new(), base));
    }
    spans
}

/// Render one conversation message in a compact, CLI-style layout:
/// thinking is dim, tool calls show their command in gray with a brief
/// result summary, and the assistant's answer stands out at the end.
fn render_msg(out: &mut Vec<Line<'static>>, msg: &crate::app::Msg, t: &crate::theme::Theme) {
    let dim = Style::default().fg(t.muted);
    let text = Style::default().fg(t.text);
    let blank = || Line::from("");
    match msg.kind {
        MsgKind::User => {
            out.push(Line::from(Span::styled(
                "› you".to_string(),
                Style::default().fg(t.user).add_modifier(Modifier::BOLD),
            )));
            for row in msg.body.lines() {
                out.push(Line::from(Span::styled(row.to_string(), text)));
            }
            out.push(blank());
        }
        MsgKind::Thinking => {
            out.push(Line::from(Span::styled(
                "✻ Thinking…".to_string(),
                Style::default().fg(t.muted).add_modifier(Modifier::ITALIC),
            )));
            for row in msg.body.lines() {
                out.push(Line::from(Span::styled(
                    format!("  {row}"),
                    Style::default().fg(t.muted).add_modifier(Modifier::ITALIC),
                )));
            }
            out.push(blank());
        }
        MsgKind::Burst => {
            let n = crate::ir::Burst::parse(&msg.body)
                .map(|b| b.actions.len())
                .unwrap_or(0);
            out.push(Line::from(vec![
                Span::styled("● ", Style::default().fg(t.accent)),
                Span::styled(
                    format!(
                        "{} · {} step{}",
                        msg.title,
                        n,
                        if n == 1 { "" } else { "s" }
                    ),
                    dim,
                ),
            ]));
        }
        MsgKind::Step { ok } => {
            let color = if ok { t.ok } else { t.err };
            let (name, args) = tool_display(&msg.title);
            // Claude-style header: ● Name(args)
            let mut header = vec![
                Span::styled("⏺ ", Style::default().fg(color)),
                Span::styled(name.clone(), Style::default().fg(t.text).add_modifier(Modifier::BOLD)),
            ];
            if !args.is_empty() {
                header.push(Span::styled(format!("({args})"), dim));
            }
            out.push(Line::from(header));
            // Result: ⎿ <smart summary>, then a short output preview.
            let (summary, preview) = tool_result(&name, &msg.body, ok);
            out.push(Line::from(Span::styled(format!("  ⎿ {summary}"), dim)));
            for l in preview.iter().take(6) {
                out.push(Line::from(Span::styled(format!("     {l}"), dim)));
            }
            if preview.len() > 6 {
                out.push(Line::from(Span::styled(
                    format!("     … +{} lines", preview.len() - 6),
                    dim,
                )));
            }
            out.push(blank());
        }
        MsgKind::Assert { ok } => {
            let color = if ok { t.ok } else { t.err };
            let glyph = if ok { "✓" } else { "✗" };
            out.push(Line::from(vec![
                Span::styled(format!("  {glyph} "), Style::default().fg(color)),
                Span::styled(msg.title.clone(), dim),
            ]));
            let detail = msg.body.trim();
            if !detail.is_empty() {
                for l in detail.lines().take(2) {
                    out.push(Line::from(Span::styled(format!("     {l}"), dim)));
                }
            }
        }
        MsgKind::Assistant => {
            let mut md: Vec<Line<'static>> = Vec::new();
            push_markdown(&mut md, &msg.body, t, "  ");
            // Put the assistant bullet on the first rendered line.
            if let Some(first) = md.first_mut() {
                let mut spans = vec![Span::styled(
                    "⏺ ",
                    Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
                )];
                spans.extend(std::mem::take(&mut first.spans));
                *first = Line::from(spans);
            } else {
                md.push(Line::from(Span::styled("⏺", Style::default().fg(t.accent))));
            }
            out.extend(md);
            out.push(blank());
        }
        MsgKind::System => {
            out.push(Line::from(Span::styled(
                format!("● {}", msg.title),
                Style::default().fg(t.accent),
            )));
            push_markdown(out, &msg.body, t, "  ");
            out.push(blank());
        }
        MsgKind::Agent => {
            out.push(Line::from(vec![
                Span::styled("⏺ ", Style::default().fg(t.ice)),
                Span::styled("Task", Style::default().fg(t.text).add_modifier(Modifier::BOLD)),
                Span::styled(format!("({})", msg.title), dim),
            ]));
            for row in msg.body.lines().take(8) {
                out.push(Line::from(Span::styled(format!("  ⎿ {row}"), dim)));
            }
            out.push(blank());
        }
        MsgKind::Loop => {
            out.push(Line::from(vec![
                Span::styled("↻ ", Style::default().fg(t.warn)),
                Span::styled(msg.title.clone(), Style::default().fg(t.warn)),
            ]));
            for row in msg.body.lines() {
                out.push(Line::from(Span::styled(format!("  {row}"), dim)));
            }
            out.push(blank());
        }
        MsgKind::Goal => {
            out.push(Line::from(vec![
                Span::styled("◎ ", Style::default().fg(t.accent).add_modifier(Modifier::BOLD)),
                Span::styled(msg.title.clone(), Style::default().fg(t.accent)),
            ]));
            for row in msg.body.lines() {
                out.push(Line::from(Span::styled(format!("  {row}"), text)));
            }
            out.push(blank());
        }
        MsgKind::Error => {
            out.push(Line::from(Span::styled(
                format!("✗ {}", msg.title),
                Style::default().fg(t.err).add_modifier(Modifier::BOLD),
            )));
            for row in msg.body.lines() {
                out.push(Line::from(Span::styled(format!("  {row}"), Style::default().fg(t.err))));
            }
            out.push(blank());
        }
    }
}

fn draw_input(f: &mut Frame, app: &App, area: Rect) {
    let t = app.theme;
    let focused = app.input_focused && !app.running;
    let title = if app.running {
        " ◈ WORKING · your next instruction can wait here "
    } else if app.screen == Screen::Onboard && app.onboard_step == 1 {
        " CONNECT · API key (masked) "
    } else {
        " > YOUR NEXT MOVE "
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(if focused {
            t.border_focus
        } else if app.running {
            t.warn
        } else {
            t.border
        }))
        .title(Span::styled(title, Style::default().fg(t.muted)));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let placeholder = if app.screen == Screen::Onboard && app.onboard_step == 1 {
        "Paste your provider key here…"
    } else {
        "What would you like to build?  /help for commands"
    };
    let show = if app.input.is_empty() && !app.running {
        placeholder.to_string()
    } else if app.screen == Screen::Onboard && app.onboard_step == 1 && !app.input.is_empty() {
        "•".repeat(app.input.chars().count().min(48))
    } else {
        app.input.clone()
    };
    let prefix = "> ";
    let color = if app.input.is_empty() {
        t.muted
    } else {
        t.text
    };
    // Render a horizontally scrolling editor. Cursor is a UTF-8 byte index,
    // but terminal positioning must use display cells (CJK, emoji, combining marks).
    let masked = app.screen == Screen::Onboard && app.onboard_step == 1;
    let cursor_width = if masked {
        app.input[..app.cursor].chars().count().min(48)
    } else {
        UnicodeWidthStr::width(&app.input[..app.cursor])
    };
    let available = inner.width.saturating_sub(3) as usize;
    let skip = cursor_width.saturating_sub(available);
    let mut cells = 0;
    let visible: String = show
        .chars()
        .skip_while(|c| {
            if cells >= skip {
                false
            } else {
                cells += unicode_width::UnicodeWidthChar::width(*c).unwrap_or(0);
                true
            }
        })
        .collect();
    f.render_widget(
        Paragraph::new(format!("{prefix}{visible}")).style(Style::default().fg(color)),
        inner,
    );
    if inner.height > 1 && !masked {
        f.render_widget(
            Paragraph::new("  Enter send   / commands   Ctrl+L help   Esc home").style(t.dim()),
            Rect::new(inner.x, inner.y + 1, inner.width, 1),
        );
    }
    if focused && inner.width > 2 && inner.height > 0 && !app.help {
        f.set_cursor(
            inner.x + 2 + cursor_width.saturating_sub(cells).min(available) as u16,
            inner.y,
        );
    }
}

fn draw_todos(f: &mut Frame, app: &App, area: Rect) {
    let t = app.theme;
    let block = Block::default()
        .borders(Borders::LEFT)
        .border_style(Style::default().fg(t.border));
    let inner = inset(block.inner(area), 2, 1);
    f.render_widget(block, area);
    let mut lines = vec![
        Line::from(Span::styled("WORKSPACE", t.title())),
        Line::from(""),
        Line::from(Span::styled(&app.provider, t.body())),
        Line::from(Span::styled(&app.model, t.dim())),
        Line::from(""),
        Line::from(Span::styled("EXECUTION", t.title())),
        Line::from(format!(
            "{:02} steps   {:02} turns",
            app.steps_ok + app.steps_fail,
            app.turns
        )),
        Line::from(vec![
            Span::styled(format!("{} ok", app.steps_ok), Style::default().fg(t.ok)),
            Span::styled(
                format!(" / {} failed", app.steps_fail),
                Style::default().fg(if app.steps_fail > 0 { t.err } else { t.muted }),
            ),
        ]),
        Line::from(format!("{} agents active", app.agents_alive)),
        Line::from(""),
        Line::from(Span::styled("TASKS", t.title())),
    ];
    if app.todos.is_empty() {
        lines.push(Line::from(Span::styled(
            "A little space for a big idea.",
            t.dim(),
        )));
        lines.push(Line::from(Span::styled(
            "/todo add your first step",
            t.dim(),
        )));
    }
    for td in &app.todos {
        lines.push(Line::from(Span::styled(
            format!("{} {} {}", if td.done { "✓" } else { "□" }, td.id, td.text),
            Style::default().fg(if td.done { t.ok } else { t.text }),
        )));
    }
    if let Some(goal) = &app.goal_text {
        lines.extend([
            Line::from(""),
            Line::from(Span::styled("NORTH STAR", t.title())),
            Line::from(goal.as_str()),
        ]);
    }
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
}

fn draw_context(f: &mut Frame, app: &App, area: Rect) {
    let t = app.theme;
    let win = app.context_window.max(1);
    let used = app.context_used.min(win);
    let ratio = used as f64 / win as f64;
    let label = format!(
        " context  {used}/{win} tok  ·  {:.0}%  ·  theme {} ",
        ratio * 100.0,
        app.theme_name.id()
    );
    let g = Gauge::default()
        .gauge_style(Style::default().fg(t.ice).bg(t.surface))
        .ratio(ratio.clamp(0.0, 1.0))
        .label(label)
        .style(Style::default().fg(t.muted).bg(t.surface));
    f.render_widget(g, area);
}

fn draw_status(f: &mut Frame, app: &App, area: Rect) {
    let t = app.theme;
    let spin = ["◐", "◓", "◑", "◒"][(app.tick / 2 % 4) as usize];
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                format!(" {} ", if app.running { spin } else { "●" }),
                Style::default().fg(if app.running { t.ice } else { t.ok }),
            ),
            Span::styled(app.status.clone(), t.dim()),
        ])),
        area,
    );
}

fn draw_help(f: &mut Frame, app: &App) {
    let t = app.theme;
    let area = centered(f.size(), 70, 22);
    f.render_widget(Clear, area);
    let block = Block::default()
        .title(Span::styled(" ICE / COMMAND GUIDE · Esc close ", t.title()))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(t.border_focus))
        .style(Style::default().bg(t.surface).fg(t.text));
    let text = vec![
        Line::from("  /theme light|ice|groknight|frost|ember|mono|solarized"),
        Line::from("  web_search <q> · web_fetch <url>   agentic web access"),
        Line::from("  /skills /mcp /todo    skills, MCP tools, live todos"),
        Line::from("  /onboard              provider + key + model wizard"),
        Line::from("  /providers [id]      list or switch xai openai groq …"),
        Line::from("  /models              discover models from the live API"),
        Line::from("  /clis                detect grok/claude/aider/codex…"),
        Line::from("  /think               toggle thinking print"),
        Line::from("  /model NAME          set model"),
        Line::from("  /goal /loop /agent   durable goal, ralph loop, subagents"),
        Line::from("  /demo /live          offline burst ↔ API"),
        Line::from("  /update              fetch & install latest release"),
        Line::from("  /exit                leave"),
        Line::from(""),
        Line::from(Span::styled(
            "  ICE: Intent → Compile → Execute → Verify → Δ",
            Style::default().fg(t.ice),
        )),
    ];
    f.render_widget(
        Paragraph::new(text).wrap(Wrap { trim: true }).block(block),
        area,
    );
}

fn inset(area: Rect, x: u16, y: u16) -> Rect {
    Rect {
        x: area.x.saturating_add(x),
        y: area.y.saturating_add(y),
        width: area.width.saturating_sub(x.saturating_mul(2)),
        height: area.height.saturating_sub(y.saturating_mul(2)),
    }
}

fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect {
        x: area.x + (area.width.saturating_sub(w)) / 2,
        y: area.y + (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    }
}

fn short_path(p: &std::path::Path) -> String {
    let s = p.display().to_string();
    if s.chars().count() <= 28 {
        s
    } else {
        format!(
            "…{}",
            s.chars().skip(s.chars().count() - 26).collect::<String>()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, Terminal};

    #[test]
    fn all_screens_render_at_responsive_sizes() {
        let mut app = App::new(std::path::PathBuf::from("C:/工作/مشروع/ice"), true);
        for (w, h) in [
            (1, 1),
            (35, 13),
            (36, 14),
            (60, 20),
            (80, 24),
            (120, 40),
            (180, 50),
        ] {
            let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
            for screen in [Screen::Welcome, Screen::Onboard, Screen::Session] {
                app.screen = screen;
                for help in [false, true] {
                    app.help = help;
                    term.draw(|f| draw(f, &app)).unwrap();
                }
            }
        }
    }

    #[test]
    fn unicode_editor_scroll_and_masked_key_remain_in_bounds() {
        let mut app = App::new(std::path::PathBuf::from("."), true);
        app.screen = Screen::Session;
        app.input = "项目 café 🧊 ".repeat(30);
        app.cursor = app.input.len();
        let mut term = Terminal::new(TestBackend::new(60, 20)).unwrap();
        term.draw(|f| draw(f, &app)).unwrap();
        app.screen = Screen::Onboard;
        app.onboard_step = 1;
        app.input = "SECRET-KEY-NEVER-RENDER".into();
        app.cursor = app.input.len();
        term.draw(|f| draw(f, &app)).unwrap();
        let text: String = term
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(!text.contains("SECRET-KEY"));
    }

    #[test]
    fn mascot_blinks_and_moves() {
        let mut app = App::new(std::path::PathBuf::from("."), true);
        app.screen = Screen::Welcome;
        let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
        term.draw(|f| draw(f, &app)).unwrap();
        let first = term.backend().buffer().clone();
        app.tick = 59;
        term.draw(|f| draw(f, &app)).unwrap();
        assert_ne!(&first, term.backend().buffer());
    }

    #[test]
    fn export_review_frames_when_requested() {
        let Some(dir) = std::env::var_os("ICE_CAPTURE_DIR") else {
            return;
        };
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = App::new(std::path::PathBuf::from("~/workspace/ice"), true);
        app.provider = "openai".into();
        app.model = "your-configured-model".into();
        app.status = "Ready when you are.  /onboard to connect a provider".into();
        app.screen = Screen::Welcome;
        let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
        for (name, screen) in [
            ("welcome", Screen::Welcome),
            ("connect", Screen::Onboard),
            ("session", Screen::Session),
        ] {
            app.screen = screen;
            if screen == Screen::Session {
                app.messages = vec![
                    crate::app::Msg { kind: MsgKind::User, title: "Give this project a clear direction.".into(), body: "Explore the workspace and map the next steps.".into() },
                    crate::app::Msg { kind: MsgKind::Thinking, title: "Understanding the workspace".into(), body: "Reading the structure, then choosing the smallest useful next step.".into() },
                    crate::app::Msg { kind: MsgKind::Step { ok: true }, title: "Workspace explored".into(), body: "src/   brand/   tools/   Cargo.toml".into() },
                    crate::app::Msg { kind: MsgKind::Assistant, title: "A clear path forward.".into(), body: "The workspace is ready. Describe the outcome you want,\nand we can turn it into a focused execution plan.".into() },
                ];
                app.steps_ok = 1;
                app.turns = 1;
            }
            let mut frames = Vec::new();
            for tick in (0..64).step_by(2) {
                app.tick = tick;
                term.draw(|f| draw(f, &app)).unwrap();
                let cells: Vec<_> = term
                    .backend()
                    .buffer()
                    .content
                    .iter()
                    .map(|c| {
                        let rgb = |color| match color {
                            ratatui::style::Color::Rgb(r, g, b) => {
                                format!("#{r:02x}{g:02x}{b:02x}")
                            }
                            _ => "#061018".to_string(),
                        };
                        serde_json::json!([c.symbol(), rgb(c.fg), rgb(c.bg)])
                    })
                    .collect();
                frames.push(cells);
            }
            std::fs::write(
                dir.join(format!("{name}.json")),
                serde_json::to_vec(&frames).unwrap(),
            )
            .unwrap();
        }
    }
}
