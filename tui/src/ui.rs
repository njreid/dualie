/// ui.rs — Ratatui rendering.

use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{
        Block, Borders, Cell, Paragraph, Row, Scrollbar, ScrollbarOrientation,
        ScrollbarState, Table, Tabs, Wrap,
    },
};

use crate::{App, SyncTabState, Tab};

// ── Colour palette ────────────────────────────────────────────────────────────

const C_ACCENT:  Color = Color::Cyan;
const C_OK:      Color = Color::Green;
const C_WARN:    Color = Color::Yellow;
const C_DIM:     Color = Color::DarkGray;
const C_HEADING: Color = Color::White;

// ── Top-level render ──────────────────────────────────────────────────────────

pub fn render(f: &mut Frame, app: &App) {
    let area = f.area();

    // Layout: tab bar | content | footer
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // tab bar
            Constraint::Min(0),     // content
            Constraint::Length(1),  // footer
        ])
        .split(area);

    render_tabs(f, app, chunks[0]);

    match app.tab {
        Tab::Status    => render_status(f, app, chunks[1]),
        Tab::Remaps    => render_remaps(f, app, chunks[1]),
        Tab::CapsLayer => render_caps_layer(f, app, chunks[1]),
        Tab::Config    => render_config(f, app, chunks[1]),
        Tab::Sync      => render_sync(f, &app.sync, chunks[1]),
    }

    render_footer(f, app, chunks[2]);
}

// ── Tab bar ───────────────────────────────────────────────────────────────────

fn render_tabs(f: &mut Frame, app: &App, area: Rect) {
    let titles: Vec<Line> = Tab::ALL.iter().map(|t| {
        Line::from(format!(" {} ", t.title()))
    }).collect();

    let tabs = Tabs::new(titles)
        .select(app.tab.index())
        .block(Block::default().borders(Borders::ALL).title(" Dualie "))
        .highlight_style(
            Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD)
        )
        .divider("|");

    f.render_widget(tabs, area);
}

// ── Footer ────────────────────────────────────────────────────────────────────

fn render_footer(f: &mut Frame, app: &App, area: Rect) {
    let msg = if let Some(m) = &app.message {
        Span::styled(m.as_str(), Style::default().fg(C_WARN))
    } else {
        let hints = match app.tab {
            Tab::Status    => " q:quit  Tab:next  r:refresh  p:pull  u:push ",
            Tab::Remaps    => " q:quit  Tab:next  a/b:output  p:pull  u:push ",
            Tab::CapsLayer => " q:quit  Tab:next  a/b:output  p:pull  u:push ",
            Tab::Config    => " q:quit  Tab:next  r:reload  e:edit in $EDITOR  p:pull  u:push ",
            Tab::Sync      => {
                if app.sync.dirty {
                    " q:quit  Tab:next  j/k:navigate  space:toggle  w:save*  p:pull  u:push "
                } else {
                    " q:quit  Tab:next  j/k:navigate  space:toggle  w:save  p:pull  u:push "
                }
            }
        };
        Span::styled(hints, Style::default().fg(C_DIM))
    };
    let p = Paragraph::new(Line::from(msg)).alignment(Alignment::Center);
    f.render_widget(p, area);
}

// ── Status tab ────────────────────────────────────────────────────────────────

fn render_status(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Status ");

    let inner = block.inner(area);
    f.render_widget(block, area);

    if let Some(ref err) = app.status_err {
        let p = Paragraph::new(format!("Cannot reach daemon:\n\n  {err}\n\nSocket: {}\n\nPress r to retry.", app.socket_path))
            .style(Style::default().fg(C_WARN))
            .wrap(Wrap { trim: false });
        f.render_widget(p, inner);
        return;
    }

    let lines: Vec<Line> = if let Some(ref st) = app.status {
        let serial_color = if st.serial == "connected" { C_OK } else { C_WARN };
        let mut lines = vec![
            Line::from(vec![
                Span::styled("  Version  ", Style::default().fg(C_DIM)),
                Span::raw(&st.version),
            ]),
            Line::from(vec![
                Span::styled("  PID      ", Style::default().fg(C_DIM)),
                Span::raw(st.pid.to_string()),
            ]),
            Line::from(vec![
                Span::styled("  Serial   ", Style::default().fg(C_DIM)),
                Span::styled(&st.serial, Style::default().fg(serial_color).add_modifier(Modifier::BOLD)),
            ]),
            Line::from(vec![
                Span::styled("  Config   ", Style::default().fg(C_DIM)),
                Span::raw(&st.config),
            ]),
            Line::from(vec![
                Span::styled("  Socket   ", Style::default().fg(C_DIM)),
                Span::raw(&app.socket_path),
            ]),
        ];

        // Git status row.
        if !st.repo_dir.is_empty() {
            lines.push(Line::from(vec![
                Span::styled("  Repo     ", Style::default().fg(C_DIM)),
                Span::raw(&st.repo_dir),
            ]));
        }

        lines.push(Line::raw(""));

        if app.git_pending > 0 {
            lines.push(Line::from(vec![
                Span::styled("  Git      ", Style::default().fg(C_DIM)),
                Span::styled(
                    format!("↓ {} commit(s) available — press p to pull", app.git_pending),
                    Style::default().fg(C_WARN).add_modifier(Modifier::BOLD),
                ),
            ]));
        } else if !st.repo_dir.is_empty() {
            lines.push(Line::from(vec![
                Span::styled("  Git      ", Style::default().fg(C_DIM)),
                Span::styled("up to date", Style::default().fg(C_OK)),
            ]));
        }

        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            format!("  Last refresh: {}s ago", app.last_refresh.elapsed().as_secs()),
            Style::default().fg(C_DIM),
        )));
        lines
    } else {
        vec![Line::from(Span::styled("  Fetching…", Style::default().fg(C_DIM)))]
    };

    let p = Paragraph::new(Text::from(lines));
    f.render_widget(p, inner);
}

// ── Remaps tab ────────────────────────────────────────────────────────────────

fn render_remaps(f: &mut Frame, app: &App, area: Rect) {
    let label = if app.output_idx == 0 { "A" } else { "B" };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" Remaps — Output {label} (press a/b to switch) "));

    let inner = block.inner(area);
    f.render_widget(block, area);

    // Parse key remaps and modifier remaps from the config text.
    let (key_rows, mod_rows) = extract_remaps(&app.config_text, app.output_idx);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(inner);

    // Key remaps table
    let key_header = Row::new(["Source key", "Destination key", "Req. modifier"])
        .style(Style::default().fg(C_HEADING).add_modifier(Modifier::BOLD | Modifier::UNDERLINED));
    let key_table = Table::new(key_rows, [
        Constraint::Percentage(33),
        Constraint::Percentage(33),
        Constraint::Percentage(34),
    ])
    .header(key_header)
    .block(Block::default().borders(Borders::BOTTOM).title(" Key remaps "));

    // Modifier remaps table
    let mod_header = Row::new(["Source modifier", "Destination modifier"])
        .style(Style::default().fg(C_HEADING).add_modifier(Modifier::BOLD | Modifier::UNDERLINED));
    let mod_table = Table::new(mod_rows, [
        Constraint::Percentage(50),
        Constraint::Percentage(50),
    ])
    .header(mod_header)
    .block(Block::default().borders(Borders::NONE).title(" Modifier remaps "));

    f.render_widget(key_table, chunks[0]);
    f.render_widget(mod_table, chunks[1]);
}

// ── Caps layer tab ───────────────────────────────────────────────────────────

fn render_caps_layer(f: &mut Frame, app: &App, area: Rect) {
    let label = if app.output_idx == 0 { "A" } else { "B" };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" Caps Layer — Output {label} (press a/b to switch) "));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let rows = extract_caps_layer(&app.config_text, app.output_idx);
    let row_count = rows.len();

    let header = Row::new(["Type", "Caps+key", "Action / target"])
        .style(Style::default().fg(C_HEADING).add_modifier(Modifier::BOLD | Modifier::UNDERLINED));

    let table = Table::new(rows, [
        Constraint::Length(10),
        Constraint::Length(12),
        Constraint::Min(0),
    ])
    .header(header)
    .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED))
    .block(Block::default().borders(Borders::NONE));

    // Scrollable via app.scroll (used as TableState offset approximation).
    let visible = inner.height.saturating_sub(2) as usize;
    let offset = (app.scroll as usize).min(row_count.saturating_sub(visible));
    let mut state = ratatui::widgets::TableState::default().with_offset(offset);
    f.render_stateful_widget(table, inner, &mut state);

    // Scrollbar
    if row_count > visible {
        let mut sb_state = ScrollbarState::new(row_count).position(offset);
        f.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight),
            inner,
            &mut sb_state,
        );
    }
}

// ── Config tab ────────────────────────────────────────────────────────────────

fn render_config(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" Config — {} ", app.config_path.display()));

    let inner = block.inner(area);
    f.render_widget(block, area);

    if app.config_text.is_empty() {
        let p = Paragraph::new("  (no config file found — press e to create one with $EDITOR)")
            .style(Style::default().fg(C_DIM));
        f.render_widget(p, inner);
        return;
    }

    // Syntax-highlight: comments grey, node names cyan, strings green.
    let lines: Vec<Line> = app.config_text.lines().map(|line| {
        if line.trim_start().starts_with("//") {
            Line::from(Span::styled(line.to_owned(), Style::default().fg(C_DIM)))
        } else if line.contains('{') || line.contains('}') {
            Line::from(Span::styled(line.to_owned(), Style::default().fg(C_HEADING)))
        } else {
            // Colour first token (node name) in accent, rest plain.
            let mut parts = line.splitn(2, ' ');
            let name = parts.next().unwrap_or("");
            let rest = parts.next().unwrap_or("");
            if name.is_empty() {
                Line::from(line.to_owned())
            } else {
                Line::from(vec![
                    Span::styled(name.to_owned(), Style::default().fg(C_ACCENT)),
                    Span::raw(if rest.is_empty() { String::new() } else { format!(" {rest}") }),
                ])
            }
        }
    }).collect();

    let total = lines.len() as u16;
    let visible = inner.height;
    let scroll = app.scroll.min(total.saturating_sub(visible));

    let p = Paragraph::new(Text::from(lines))
        .scroll((scroll, 0));
    f.render_widget(p, inner);

    // Scrollbar
    if total > visible {
        let mut sb_state = ScrollbarState::new(total as usize).position(scroll as usize);
        f.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight),
            inner,
            &mut sb_state,
        );
    }
}

// ── Sync tab ──────────────────────────────────────────────────────────────────

pub fn render_sync(f: &mut Frame, state: &SyncTabState, area: Rect) {
    let dirty_marker = if state.dirty { " [unsaved]" } else { "" };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" Sync — App registry{dirty_marker} "));

    let inner = block.inner(area);
    f.render_widget(block, area);

    if state.rows.is_empty() {
        let p = Paragraph::new("  No apps in registry.")
            .style(Style::default().fg(C_DIM));
        f.render_widget(p, inner);
        return;
    }

    // Split: left list (app names) | right detail (file globs for selected app).
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(inner);

    // ── Left: app list ────────────────────────────────────────────────────────

    let list_rows: Vec<Row> = state.rows.iter().enumerate().map(|(i, row)| {
        let check = if row.enabled { "[x]" } else { "[ ]" };
        let check_style = if row.enabled {
            Style::default().fg(C_OK)
        } else {
            Style::default().fg(C_DIM)
        };
        let label_style = if i == state.selected {
            Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD)
        } else if row.enabled {
            Style::default()
        } else {
            Style::default().fg(C_DIM)
        };
        Row::new([
            Cell::from(check.to_owned()).style(check_style),
            Cell::from(row.entry.label.clone()).style(label_style),
        ])
    }).collect();

    let header = Row::new(["", "App"])
        .style(Style::default().fg(C_HEADING).add_modifier(Modifier::BOLD | Modifier::UNDERLINED));

    let list_table = Table::new(list_rows, [Constraint::Length(3), Constraint::Min(0)])
        .header(header)
        .block(Block::default().borders(Borders::RIGHT))
        .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED));

    let visible = chunks[0].height.saturating_sub(2) as usize;
    let offset = state.selected.saturating_sub(visible.saturating_sub(1).max(1) / 2)
        .min(state.rows.len().saturating_sub(visible));
    let mut table_state = ratatui::widgets::TableState::default()
        .with_selected(Some(state.selected))
        .with_offset(offset);
    f.render_stateful_widget(list_table, chunks[0], &mut table_state);

    // ── Right: detail for selected app ────────────────────────────────────────

    if let Some(row) = state.rows.get(state.selected) {
        let detail_block = Block::default()
            .borders(Borders::NONE)
            .title(format!(" {} ", row.entry.label));
        let detail_inner = detail_block.inner(chunks[1]);
        f.render_widget(detail_block, chunks[1]);

        let mut lines: Vec<Line> = Vec::new();

        // Comment char.
        if !row.entry.comment_char.is_empty() {
            lines.push(Line::from(vec![
                Span::styled("  comment  ", Style::default().fg(C_DIM)),
                Span::styled(
                    format!("\"{}\"", row.entry.comment_char),
                    Style::default().fg(C_ACCENT),
                ),
            ]));
        } else {
            lines.push(Line::from(vec![
                Span::styled("  comment  ", Style::default().fg(C_DIM)),
                Span::styled("(none — JSON/binary format)", Style::default().fg(C_DIM)),
            ]));
        }
        lines.push(Line::raw(""));

        // File globs / actual paths.
        if row.entry.globs.is_empty() {
            lines.push(Line::from(
                Span::styled("  (no files for this platform)", Style::default().fg(C_DIM))
            ));
        } else {
            lines.push(Line::from(
                Span::styled("  Config files:", Style::default().fg(C_HEADING))
            ));
            for glob in &row.entry.globs {
                // Show actual expanded paths if they exist, otherwise the raw glob.
                let expanded = row.entry.expand_globs();
                if expanded.is_empty() {
                    lines.push(Line::from(vec![
                        Span::styled("    ", Style::default()),
                        Span::styled(glob.clone(), Style::default().fg(C_DIM)),
                        Span::styled("  (not found)", Style::default().fg(C_DIM)),
                    ]));
                } else {
                    // Show only paths that match this particular glob pattern.
                    let home = directories::UserDirs::new()
                        .map(|d| d.home_dir().to_path_buf());
                    for path in &expanded {
                        let display = if let Some(ref h) = home {
                            path.strip_prefix(h)
                                .map(|p| format!("~/{}", p.display()))
                                .unwrap_or_else(|_| path.display().to_string())
                        } else {
                            path.display().to_string()
                        };
                        lines.push(Line::from(vec![
                            Span::styled("    ", Style::default()),
                            Span::styled(display, Style::default().fg(C_OK)),
                        ]));
                    }
                    break; // expanded already covers all globs for this platform
                }
            }
        }

        if row.enabled {
            lines.push(Line::raw(""));
            lines.push(Line::from(
                Span::styled("  ✓ enabled for sync", Style::default().fg(C_OK))
            ));
        }

        let p = Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false });
        f.render_widget(p, detail_inner);
    }
}

// ── Config text parsing helpers ───────────────────────────────────────────────

/// Very lightweight line scanner — not a real KDL parser.
/// Finds the `output A/B { ... }` block and scans `remap { key/modifier ... }`.

fn extract_remaps(src: &str, output_idx: usize) -> (Vec<Row<'static>>, Vec<Row<'static>>) {
    let label = if output_idx == 0 { "A" } else { "B" };
    let mut key_rows: Vec<Row<'static>> = Vec::new();
    let mut mod_rows: Vec<Row<'static>> = Vec::new();
    let mut in_output = false;
    let mut depth: i32 = 0;
    let mut in_remap = false;

    for raw in src.lines() {
        let line = raw.trim();

        // Detect `output A {`
        if !in_output {
            if line.starts_with("output") && line.contains(label) && line.contains('{') {
                in_output = true;
                depth = 1;
            }
            continue;
        }

        // Track brace depth to know when we've left the output block.
        let opens  = line.chars().filter(|&c| c == '{').count() as i32;
        let closes = line.chars().filter(|&c| c == '}').count() as i32;

        if in_remap {
            if closes > 0 && depth - closes < 2 {
                in_remap = false;
            } else if line.starts_with("key") {
                let parts: Vec<&str> = line.splitn(4, ' ').collect();
                let src_k  = parts.get(1).unwrap_or(&"?").to_string();
                let dst_k  = parts.get(2).unwrap_or(&"?").to_string();
                let req_mod = if let Some(p) = parts.get(3) {
                    p.trim_start_matches("src-mod=").to_string()
                } else {
                    String::new()
                };
                key_rows.push(Row::new([src_k, dst_k, req_mod]));
            } else if line.starts_with("modifier") {
                let parts: Vec<&str> = line.splitn(3, ' ').collect();
                let src_m = parts.get(1).unwrap_or(&"?").to_string();
                let dst_m = parts.get(2).unwrap_or(&"?").to_string();
                mod_rows.push(Row::new([src_m, dst_m]));
            }
        } else if line.starts_with("remap") && line.contains('{') {
            in_remap = true;
        }

        depth += opens - closes;
        if depth <= 0 {
            break;
        }
    }

    if key_rows.is_empty() {
        key_rows.push(Row::new(["(none)", "", ""]).style(Style::default().fg(C_DIM)));
    }
    if mod_rows.is_empty() {
        mod_rows.push(Row::new(["(none)", ""]).style(Style::default().fg(C_DIM)));
    }

    (key_rows, mod_rows)
}

fn extract_caps_layer(src: &str, output_idx: usize) -> Vec<Row<'static>> {
    let label = if output_idx == 0 { "A" } else { "B" };
    let mut rows: Vec<Row<'static>> = Vec::new();
    let mut in_output = false;
    let mut in_caps = false;
    let mut depth: i32 = 0;

    for raw in src.lines() {
        let line = raw.trim();

        if !in_output {
            if line.starts_with("output") && line.contains(label) && line.contains('{') {
                in_output = true;
                depth = 1;
            }
            continue;
        }

        let opens  = line.chars().filter(|&c| c == '{').count() as i32;
        let closes = line.chars().filter(|&c| c == '}').count() as i32;

        if in_caps {
            if closes > 0 && depth - closes < 3 {
                in_caps = false;
            } else {
                let (entry_type, rest_style) = if line.starts_with("chord") {
                    ("chord", C_OK)
                } else if line.starts_with("action") {
                    ("action", C_ACCENT)
                } else if line.starts_with("jump-a") {
                    ("jump-a", C_WARN)
                } else if line.starts_with("jump-b") {
                    ("jump-b", C_WARN)
                } else if line.starts_with("swap") {
                    ("swap", Color::Magenta)
                } else {
                    ("", C_DIM)
                };

                if !entry_type.is_empty() {
                    let parts: Vec<&str> = line.splitn(3, ' ').collect();
                    let src_key = parts.get(1).unwrap_or(&"").to_string();
                    let target  = parts.get(2).unwrap_or(&"").to_string();
                    rows.push(Row::new([
                        Cell::from(entry_type.to_owned()).style(Style::default().fg(rest_style)),
                        Cell::from(src_key),
                        Cell::from(target),
                    ]));
                }
            }
        } else if line.starts_with("caps") && line.contains('{') {
            in_caps = true;
        }

        depth += opens - closes;
        if depth <= 0 {
            break;
        }
    }

    if rows.is_empty() {
        rows.push(Row::new(["(none)", "", ""]).style(Style::default().fg(C_DIM)));
    }
    rows
}
