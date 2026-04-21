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

use crate::{ActionsMode, ActionsTabState, App, LayersMode, LAYER_ENTRY_TYPES, SyncTabState, Tab};

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
        Tab::Layers    => render_layers(f, app, chunks[1]),
        Tab::Sync      => render_sync(f, &app.sync, chunks[1]),
        Tab::Actions   => render_actions(f, &app.actions, chunks[1]),
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
            Tab::Status    => " q:quit  Tab:next  r:refresh  e:edit config  p:pull  u:push ",
            Tab::Remaps    => " q:quit  Tab:next  a/b:output  e:edit config  p:pull  u:push ",
            Tab::Layers    => match &app.layers_mode {
                LayersMode::Browse => " q:quit  Tab:next  a/b:output  h/l:layer  n:new  e:edit config  p:pull  u:push ",
                LayersMode::PickType { .. } => " j/k:navigate  Enter:select  Esc:cancel ",
                LayersMode::TypeSrc { .. } => " type key name  Enter:confirm  Esc:cancel ",
                LayersMode::TypeTarget { entry_type, .. } => if entry_type == "action" {
                    " type action label  Enter:save  Esc:cancel "
                } else {
                    " type target key(s)  Enter:save  Esc:cancel "
                },
            },
            Tab::Sync      => {
                if app.sync.dirty {
                    " q:quit  Tab:next  j/k:navigate  space:toggle  w:save*  p:pull  u:push "
                } else {
                    " q:quit  Tab:next  j/k:navigate  space:toggle  w:save  p:pull  u:push "
                }
            }
            Tab::Actions   => match app.actions.mode {
                ActionsMode::Browse => " q:quit  Tab:next  n:new action  e:edit config ",
                ActionsMode::Search => " type to filter  j/k:navigate  Enter:select  Esc:cancel ",
            },
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

        // Config parse error — shown prominently when present.
        if !st.config_error.is_empty() {
            lines.push(Line::raw(""));
            lines.push(Line::from(Span::styled(
                "  ✗ Config parse error:",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            )));
            for line in st.config_error.lines() {
                lines.push(Line::from(Span::styled(
                    format!("    {line}"),
                    Style::default().fg(Color::Red),
                )));
            }
        }

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

fn render_layers(f: &mut Frame, app: &App, area: Rect) {
    let output_label = if app.output_idx == 0 { "A" } else { "B" };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" Layers — Output {output_label} (press a/b to switch) "));
    let inner = block.inner(area);
    f.render_widget(block, area);

    // Known layers; more will be added in the future.
    const LAYERS: &[&str] = &["Caps"];

    // Split: layer-selector strip (3 rows) | table
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0)])
        .split(inner);

    // ── Layer selector ────────────────────────────────────────────────────────
    let selector_block = Block::default()
        .borders(Borders::BOTTOM)
        .title(" Layer (h/l to switch) ");
    let selector_inner = selector_block.inner(chunks[0]);
    f.render_widget(selector_block, chunks[0]);

    let tabs = ratatui::widgets::Tabs::new(LAYERS.iter().map(|l| Line::from(*l)).collect::<Vec<_>>())
        .select(app.layers_selected)
        .highlight_style(Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD))
        .divider("|");
    f.render_widget(tabs, selector_inner);

    // ── Layer content ─────────────────────────────────────────────────────────
    // Currently only "Caps" (index 0) exists.
    let layer_name = LAYERS.get(app.layers_selected).copied().unwrap_or("Caps");
    let rows = extract_layer(&app.config_text, app.output_idx, layer_name);
    let row_count = rows.len();

    let src_col = if layer_name == "Caps" { "Caps+key" } else { "Key" };
    let header = Row::new(["Type", src_col, "Action / target"])
        .style(Style::default().fg(C_HEADING).add_modifier(Modifier::BOLD | Modifier::UNDERLINED));

    let table = Table::new(rows, [
        Constraint::Length(10),
        Constraint::Length(12),
        Constraint::Min(0),
    ])
    .header(header)
    .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED))
    .block(Block::default().borders(Borders::NONE));

    let visible = chunks[1].height.saturating_sub(2) as usize;
    let offset = (app.scroll as usize).min(row_count.saturating_sub(visible));
    let mut tstate = ratatui::widgets::TableState::default().with_offset(offset);
    f.render_stateful_widget(table, chunks[1], &mut tstate);

    if row_count > visible {
        let mut sb_state = ScrollbarState::new(row_count).position(offset);
        f.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight),
            chunks[1],
            &mut sb_state,
        );
    }

    // ── Modal overlay ─────────────────────────────────────────────────────────
    match &app.layers_mode {
        LayersMode::Browse => {}
        LayersMode::PickType { type_sel } => {
            let popup = centered_popup(area, 36, LAYER_ENTRY_TYPES.len() as u16 + 4);
            let block = Block::default()
                .borders(Borders::ALL)
                .title(" Select mapping type ")
                .style(Style::default().fg(C_HEADING));
            let inner_popup = block.inner(popup);
            f.render_widget(ratatui::widgets::Clear, popup);
            f.render_widget(block, popup);

            let rows: Vec<Row> = LAYER_ENTRY_TYPES.iter().enumerate().map(|(i, t)| {
                let style = if i == *type_sel {
                    Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD | Modifier::REVERSED)
                } else {
                    Style::default()
                };
                Row::new([Cell::from(format!("  {t}  ")).style(style)])
            }).collect();
            let table = Table::new(rows, [Constraint::Min(0)])
                .block(Block::default().borders(Borders::NONE));
            f.render_widget(table, inner_popup);
        }
        LayersMode::TypeSrc { src, .. } => {
            let popup = centered_popup(area, 44, 5);
            let block = Block::default()
                .borders(Borders::ALL)
                .title(" Enter source key name ")
                .style(Style::default().fg(C_HEADING));
            let inner_popup = block.inner(popup);
            f.render_widget(ratatui::widgets::Clear, popup);
            f.render_widget(block, popup);
            let p = Paragraph::new(format!(" {src}_ "))
                .style(Style::default().fg(C_ACCENT));
            f.render_widget(p, inner_popup);
        }
        LayersMode::TypeTarget { entry_type, target, .. } => {
            let title = if entry_type == "action" {
                " Enter action label "
            } else {
                " Enter target key name(s) "
            };
            let popup = centered_popup(area, 44, 5);
            let block = Block::default()
                .borders(Borders::ALL)
                .title(title)
                .style(Style::default().fg(C_HEADING));
            let inner_popup = block.inner(popup);
            f.render_widget(ratatui::widgets::Clear, popup);
            f.render_widget(block, popup);
            let p = Paragraph::new(format!(" {target}_ "))
                .style(Style::default().fg(C_ACCENT));
            f.render_widget(p, inner_popup);
        }
    }
}

/// Return a centered `Rect` of the given width and height within `area`.
fn centered_popup(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width);
    let h = height.min(area.height);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    Rect::new(x, y, w, h)
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

// ── Actions tab ───────────────────────────────────────────────────────────────

pub fn render_actions(f: &mut Frame, state: &ActionsTabState, area: Rect) {
    match state.mode {
        ActionsMode::Browse => render_actions_browse(f, state, area),
        ActionsMode::Search => render_actions_search(f, state, area),
    }
}

fn render_actions_browse(f: &mut Frame, state: &ActionsTabState, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Actions — Virtual key bindings ");
    let inner = block.inner(area);
    f.render_widget(block, area);

    if state.rows.is_empty() {
        let p = Paragraph::new(
            "  No actions configured.\n\n  Press n to add one from installed applications."
        ).style(Style::default().fg(C_DIM));
        f.render_widget(p, inner);
        return;
    }

    let header = Row::new(["Type", "Label", "App ID / Command"])
        .style(Style::default().fg(C_HEADING).add_modifier(Modifier::BOLD | Modifier::UNDERLINED));

    let rows: Vec<Row> = state.rows.iter().enumerate().map(|(i, r)| {
        let style = if i == state.selected {
            Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        let kind_style = if r.kind == "launch" {
            Style::default().fg(C_OK)
        } else {
            Style::default().fg(C_WARN)
        };
        Row::new([
            Cell::from(r.kind.clone()).style(kind_style),
            Cell::from(r.label.clone()).style(style),
            Cell::from(r.app_id.clone()).style(Style::default().fg(C_DIM)),
        ])
    }).collect();

    let table = Table::new(rows, [
        Constraint::Length(8),
        Constraint::Percentage(35),
        Constraint::Min(0),
    ])
    .header(header)
    .block(Block::default().borders(Borders::NONE));

    let visible = inner.height.saturating_sub(2) as usize;
    let offset = state.selected.saturating_sub(visible.saturating_sub(1).max(1) / 2)
        .min(state.rows.len().saturating_sub(visible));
    let mut tstate = ratatui::widgets::TableState::default()
        .with_selected(Some(state.selected))
        .with_offset(offset);
    f.render_stateful_widget(table, inner, &mut tstate);
}

fn render_actions_search(f: &mut Frame, state: &ActionsTabState, area: Rect) {
    // Layout: search box (3 lines) | results list
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0)])
        .split(area);

    // Search box
    let search_block = Block::default()
        .borders(Borders::ALL)
        .title(" Search applications — type to filter ");
    let query_display = format!(" {} ", state.query);
    let search_p = Paragraph::new(query_display)
        .style(Style::default().fg(C_ACCENT))
        .block(search_block);
    f.render_widget(search_p, chunks[0]);

    // Results
    let results_block = Block::default()
        .borders(Borders::ALL)
        .title(" Installed applications ");
    let results_inner = results_block.inner(chunks[1]);
    f.render_widget(results_block, chunks[1]);

    let filtered = state.filtered_apps();

    if filtered.is_empty() {
        let msg = if state.all_apps.as_ref().map(|a| a.is_empty()).unwrap_or(true) {
            "  Loading…"
        } else {
            "  No matching applications."
        };
        f.render_widget(
            Paragraph::new(msg).style(Style::default().fg(C_DIM)),
            results_inner,
        );
        return;
    }

    let header = Row::new(["Name", "App ID"])
        .style(Style::default().fg(C_HEADING).add_modifier(Modifier::BOLD | Modifier::UNDERLINED));

    let rows: Vec<Row> = filtered.iter().enumerate().map(|(i, (id, name))| {
        let style = if i == state.app_sel {
            Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        Row::new([
            Cell::from(name.clone()).style(style),
            Cell::from(id.clone()).style(Style::default().fg(C_DIM)),
        ])
    }).collect();

    let table = Table::new(rows, [Constraint::Percentage(40), Constraint::Percentage(60)])
        .header(header)
        .block(Block::default().borders(Borders::NONE));

    let visible = results_inner.height.saturating_sub(2) as usize;
    let offset = state.app_sel.saturating_sub(visible.saturating_sub(1).max(1) / 2)
        .min(filtered.len().saturating_sub(visible));
    let mut tstate = ratatui::widgets::TableState::default()
        .with_selected(Some(state.app_sel))
        .with_offset(offset);
    f.render_stateful_widget(table, results_inner, &mut tstate);
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

/// Find the `caps` node inside `machine_node { layers { caps { } } }`.
fn find_caps_in_machine(machine_node: &kdl::KdlNode) -> Option<&kdl::KdlNode> {
    let layers = machine_node.children()?
        .nodes().iter()
        .find(|n| n.name().value() == "layers")?;
    layers.children()?
        .nodes().iter()
        .find(|n| n.name().value() == "caps")
}

/// Parse caps entries from a KDL `caps { }` node into table rows.
fn caps_rows_from_node(caps_node: &kdl::KdlNode) -> Vec<Row<'static>> {
    let mut rows = Vec::new();
    let Some(children) = caps_node.children() else { return rows };
    for node in children.nodes() {
        let entry_type = node.name().value().to_owned();
        let type_style = match entry_type.as_str() {
            "chord"     => Style::default().fg(C_OK),
            "action"    => Style::default().fg(C_ACCENT),
            "jump-a" | "jump-b" => Style::default().fg(C_WARN),
            "swap"      => Style::default().fg(Color::Magenta),
            "clip-pull" => Style::default().fg(C_DIM),
            _           => Style::default().fg(C_DIM),
        };
        let positional: Vec<String> = node.entries().iter()
            .filter(|e| e.name().is_none())
            .map(|e| match e.value() {
                kdl::KdlValue::String(s) => s.clone(),
                other => other.to_string(),
            })
            .collect();
        let src_key = positional.first().cloned().unwrap_or_default();
        let target  = positional[1..].join(" ");
        rows.push(Row::new([
            Cell::from(entry_type).style(type_style),
            Cell::from(src_key),
            Cell::from(target),
        ]));
    }
    rows
}

fn extract_layer(src: &str, output_idx: usize, _layer_name: &str) -> Vec<Row<'static>> {
    let port_key = if output_idx == 0 { "a" } else { "b" };
    let mut rows: Vec<Row<'static>> = Vec::new();

    let Ok(doc) = src.parse::<kdl::KdlDocument>() else {
        rows.push(Row::new(["(parse error)", "", ""]).style(Style::default().fg(C_WARN)));
        return rows;
    };

    // Step 1: resolve machine name for the selected port from `ports { a NAME; b NAME }`.
    let machine_name: Option<String> = doc.nodes().iter()
        .find(|n| n.name().value() == "ports")
        .and_then(|ports| ports.children())
        .and_then(|children| {
            children.nodes().iter()
                .find(|n| n.name().value() == port_key)
                .and_then(|n| n.entries().iter()
                    .find(|e| e.name().is_none())
                    .and_then(|e| e.value().as_string())
                    .map(|s| s.to_owned()))
        });

    // Step 2: collect from `machine *`.
    for node in doc.nodes() {
        if node.name().value() != "machine" { continue; }
        let is_wildcard = node.entries().iter()
            .find(|e| e.name().is_none())
            .and_then(|e| e.value().as_string())
            .map(|s| s == "*")
            .unwrap_or(false);
        if is_wildcard {
            if let Some(caps) = find_caps_in_machine(node) {
                rows.extend(caps_rows_from_node(caps));
            }
        }
    }

    // Step 3: collect from the named machine.
    if let Some(ref name) = machine_name {
        for node in doc.nodes() {
            if node.name().value() != "machine" { continue; }
            let node_name = node.entries().iter()
                .find(|e| e.name().is_none())
                .and_then(|e| e.value().as_string())
                .unwrap_or("");
            if node_name == name {
                if let Some(caps) = find_caps_in_machine(node) {
                    rows.extend(caps_rows_from_node(caps));
                }
            }
        }
    }

    if rows.is_empty() {
        rows.push(Row::new(["(none)", "", ""]).style(Style::default().fg(C_DIM)));
    }
    rows
}
