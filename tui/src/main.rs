/// dualie-tui — Terminal UI for the Dualie daemon.
///
/// Tabs:
///   1  Status      — live daemon status polled from the Unix socket
///   2  Remaps      — key and modifier remaps for the selected output
///   3  Caps layer  — caps-layer binding table for the selected output
///   4  Config      — raw KDL config with reload / open-in-$EDITOR
///   5  Sync        — per-app config-file sync registry; toggle apps on/off

mod app_registry;
mod ipc;
mod ui;

use std::time::{Duration, Instant};

use anyhow::Result;
use clap::Parser;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};

use app_registry::AppEntry;
use ipc::DaemonStatus;

// ── CLI ───────────────────────────────────────────────────────────────────────

#[derive(Parser, Debug)]
#[command(version, about = "Dualie terminal UI")]
struct Args {
    /// Path to the daemon status socket.
    /// Defaults to $XDG_RUNTIME_DIR/dualie/daemon.sock.
    #[arg(long)]
    socket: Option<String>,
}

// ── Tabs ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Status,
    Remaps,
    CapsLayer,
    Config,
    Sync,
}

impl Tab {
    const ALL: &'static [Tab] = &[Tab::Status, Tab::Remaps, Tab::CapsLayer, Tab::Config, Tab::Sync];

    fn title(self) -> &'static str {
        match self {
            Tab::Status    => "Status",
            Tab::Remaps    => "Remaps",
            Tab::CapsLayer => "Caps Layer",
            Tab::Config    => "Config",
            Tab::Sync      => "Sync",
        }
    }

    fn index(self) -> usize {
        Self::ALL.iter().position(|&t| t == self).unwrap_or(0)
    }

    fn next(self) -> Self {
        let i = (self.index() + 1) % Self::ALL.len();
        Self::ALL[i]
    }

    fn prev(self) -> Self {
        let i = (self.index() + Self::ALL.len() - 1) % Self::ALL.len();
        Self::ALL[i]
    }
}

// ── Sync tab state ────────────────────────────────────────────────────────────

/// One row in the sync tab: an app with its enabled state.
pub struct SyncRow {
    pub entry:   AppEntry,
    pub enabled: bool,
}

/// All state for the Sync tab.
pub struct SyncTabState {
    pub rows:     Vec<SyncRow>,
    pub selected: usize,
    /// `true` if the enabled set has been changed but not yet written to disk.
    pub dirty:    bool,
}

impl SyncTabState {
    fn load(config_text: &str) -> Self {
        let entries = app_registry::load_registry();
        let enabled_set = parse_sync_apps(config_text);
        let rows = entries.into_iter().map(|e| {
            let enabled = enabled_set.contains(&e.name);
            SyncRow { entry: e, enabled }
        }).collect();
        Self { rows, selected: 0, dirty: false }
    }

    fn toggle_selected(&mut self) {
        if let Some(row) = self.rows.get_mut(self.selected) {
            row.enabled = !row.enabled;
            self.dirty = true;
        }
    }

    fn move_up(&mut self) {
        if self.selected > 0 { self.selected -= 1; }
    }

    fn move_down(&mut self) {
        if self.selected + 1 < self.rows.len() { self.selected += 1; }
    }
}

/// Parse the `sync { app "..." ... }` block from KDL config text.
/// Returns the set of enabled app names.
fn parse_sync_apps(src: &str) -> std::collections::HashSet<String> {
    let mut set = std::collections::HashSet::new();
    if let Ok(doc) = src.parse::<kdl::KdlDocument>() {
        for node in doc.nodes() {
            if node.name().value() == "sync" {
                if let Some(children) = node.children() {
                    for child in children.nodes() {
                        if child.name().value() == "app" {
                            if let Some(name) = child.entries()
                                .iter()
                                .find(|e| e.name().is_none())
                                .and_then(|e| e.value().as_string())
                            {
                                set.insert(name.to_owned());
                            }
                        }
                    }
                }
            }
        }
    }
    set
}

/// Rewrite the `sync { }` block in `src`, returning the new KDL string.
/// If no `sync` node exists one is appended.
pub fn write_sync_apps(src: &str, enabled: &[&str]) -> String {
    // Build the replacement sync block.
    let block = if enabled.is_empty() {
        "sync {\n}\n".to_owned()
    } else {
        let inner: String = enabled.iter()
            .map(|n| format!("    app \"{n}\"\n"))
            .collect();
        format!("sync {{\n{inner}}}\n")
    };

    // Replace or append the sync node.
    if let Ok(mut doc) = src.parse::<kdl::KdlDocument>() {
        // Remove existing sync node.
        doc.nodes_mut().retain(|n| n.name().value() != "sync");
        let new_src = format!("{doc}{block}");
        return new_src;
    }
    // Fallback: just append.
    format!("{src}\n{block}")
}

// ── App state ─────────────────────────────────────────────────────────────────

pub struct App {
    pub tab:          Tab,
    pub status:       Option<DaemonStatus>,
    pub status_err:   Option<String>,
    pub config_text:  String,
    pub config_path:  std::path::PathBuf,
    pub scroll:       u16,
    pub output_idx:   usize,  // 0 = A, 1 = B
    pub sync:         SyncTabState,
    /// Queued one-line message shown in the footer (errors, info).
    pub message:      Option<String>,
    pub last_refresh: Instant,
    pub socket_path:  String,
    /// Cached pending commit count from last daemon status poll.
    pub git_pending:  u32,
}

impl App {
    fn new(socket_path: String) -> Self {
        let config_path = kdl_config_path();
        let config_text = std::fs::read_to_string(&config_path).unwrap_or_default();
        let sync = SyncTabState::load(&config_text);
        Self {
            tab:          Tab::Status,
            status:       None,
            status_err:   None,
            config_text,
            config_path,
            scroll:       0,
            output_idx:   0,
            sync,
            message:      None,
            last_refresh: Instant::now() - Duration::from_secs(10),
            socket_path,
            git_pending:  0,
        }
    }

    fn refresh_status(&mut self) {
        match ipc::query_status(&self.socket_path) {
            Ok(s) => {
                self.git_pending = s.git_pending;
                self.status = Some(s);
                self.status_err = None;
            }
            Err(e) => { self.status_err = Some(e.to_string()); }
        }
        self.last_refresh = Instant::now();
    }

    fn reload_config(&mut self) {
        match std::fs::read_to_string(&self.config_path) {
            Ok(text) => {
                self.config_text = text;
                self.sync = SyncTabState::load(&self.config_text);
                self.message = Some("Config reloaded.".into());
            }
            Err(e) => {
                self.message = Some(format!("Reload error: {e}"));
            }
        }
    }

    fn save_sync(&mut self) {
        let enabled: Vec<&str> = self.sync.rows.iter()
            .filter(|r| r.enabled)
            .map(|r| r.entry.name.as_str())
            .collect();
        let new_src = write_sync_apps(&self.config_text, &enabled);
        let path = &self.config_path;
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match std::fs::write(path, &new_src) {
            Ok(()) => {
                self.config_text = new_src;
                self.sync.dirty = false;
                self.message = Some("Sync config saved.".into());
            }
            Err(e) => {
                self.message = Some(format!("Save error: {e}"));
            }
        }
    }

    fn repo_dir(&self) -> Option<String> {
        self.status.as_ref()
            .map(|s| s.repo_dir.clone())
            .filter(|s| !s.is_empty())
    }

    /// Run `git pull --rebase origin main` in the repo, then copy the updated
    /// `dualie/dualie.kdl` to the live config path so notify fires and the
    /// daemon hot-reloads.
    ///
    /// Note: the daemon's auto-commit task (git_sync::spawn) and this copy run
    /// concurrently.  In the unlikely event that the daemon writes a new
    /// auto-commit between the git-pull and our fs::copy, the repo file could
    /// revert to the pre-pull state.  Accepted limitation for now; a future
    /// improvement would route pull/push through a daemon IPC command so that
    /// serialisation is guaranteed.
    fn git_pull(&mut self) {
        let repo_dir = match self.repo_dir() {
            Some(d) => d,
            None => {
                self.message = Some("No repo dir (daemon not connected or git not configured)".into());
                return;
            }
        };
        let out = std::process::Command::new("git")
            .args(["-C", &repo_dir, "pull", "--rebase", "origin", "main"])
            .output();
        match out {
            Ok(o) if o.status.success() => {
                // Mirror what GitRepo::pull() does: propagate repo file → live config.
                let repo_kdl = std::path::Path::new(&repo_dir)
                    .join("dualie")
                    .join("dualie.kdl");
                if repo_kdl.exists() {
                    if let Some(parent) = self.config_path.parent() {
                        if let Err(e) = std::fs::create_dir_all(parent) {
                            self.message = Some(format!("Pull: could not create config dir: {e}"));
                            return;
                        }
                    }
                    if let Err(e) = std::fs::copy(&repo_kdl, &self.config_path) {
                        self.message = Some(format!("Pull: could not write config: {e}"));
                        return;
                    }
                }
                self.reload_config();
                self.git_pending = 0;
                self.message = Some("Git pull complete — config reloaded.".into());
            }
            Ok(o) => {
                let stderr = String::from_utf8_lossy(&o.stderr);
                let first = stderr.lines().next().unwrap_or("unknown error");
                self.message = Some(format!("Pull failed: {first}"));
            }
            Err(e) => self.message = Some(format!("git: {e}")),
        }
    }

    /// Run `git push origin main` in the repo.
    fn git_push(&mut self) {
        let repo_dir = match self.repo_dir() {
            Some(d) => d,
            None => {
                self.message = Some("No repo dir (daemon not connected or git not configured)".into());
                return;
            }
        };
        let out = std::process::Command::new("git")
            .args(["-C", &repo_dir, "push", "origin", "main"])
            .output();
        match out {
            Ok(o) if o.status.success() => {
                self.message = Some("Git push complete.".into());
            }
            Ok(o) => {
                let stderr = String::from_utf8_lossy(&o.stderr);
                let first = stderr.lines().next().unwrap_or("unknown error");
                self.message = Some(format!("Push failed: {first}"));
            }
            Err(e) => self.message = Some(format!("git: {e}")),
        }
    }

    fn open_in_editor(&mut self) {
        let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".into());
        let path = self.config_path.to_str().unwrap_or("").to_owned();
        // Suspend TUI, run editor, resume.
        let _ = disable_raw_mode();
        let _ = execute!(std::io::stdout(), LeaveAlternateScreen);
        let _ = std::process::Command::new(&editor).arg(&path).status();
        let _ = enable_raw_mode();
        let _ = execute!(std::io::stdout(), EnterAlternateScreen);
        self.reload_config();
    }
}

// ── Config path (mirrors daemon logic) ───────────────────────────────────────

fn kdl_config_path() -> std::path::PathBuf {
    if let Some(proj) = directories::ProjectDirs::from("", "", "dualie") {
        proj.config_dir().join("dualie.kdl")
    } else {
        std::path::PathBuf::from("dualie.kdl")
    }
}

fn default_socket_path() -> String {
    if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        return format!("{dir}/dualie/daemon.sock");
    }
    // Can't know the daemon's pid; fall back to a glob search.
    if let Ok(entries) = std::fs::read_dir("/tmp") {
        for e in entries.flatten() {
            let name = e.file_name();
            let s = name.to_string_lossy();
            if s.starts_with("dualie-") {
                let sock = e.path().join("daemon.sock");
                if sock.exists() {
                    return sock.to_string_lossy().into_owned();
                }
            }
        }
    }
    "/tmp/dualie.sock".into()
}

// ── Event loop ────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let socket_path = args.socket.unwrap_or_else(default_socket_path);

    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_app(&mut terminal, socket_path).await;

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
    terminal.show_cursor()?;

    if let Err(e) = result {
        eprintln!("Error: {e:#}");
    }
    Ok(())
}

async fn run_app<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    socket_path: String,
) -> Result<()> {
    let mut app = App::new(socket_path);
    // Initial status fetch.
    app.refresh_status();

    let tick = Duration::from_millis(100);
    let refresh_interval = Duration::from_secs(2);

    loop {
        terminal.draw(|f| ui::render(f, &app))?;

        // Auto-refresh status on interval.
        if app.tab == Tab::Status && app.last_refresh.elapsed() >= refresh_interval {
            app.refresh_status();
        }

        // Poll crossterm events with a short timeout.
        if event::poll(tick)? {
            if let Event::Key(key) = event::read()? {
                // Clear message on any keypress.
                app.message = None;

                match (key.modifiers, key.code) {
                    // Quit
                    (_, KeyCode::Char('q')) | (KeyModifiers::CONTROL, KeyCode::Char('c')) => {
                        return Ok(());
                    }
                    // Tab navigation
                    (_, KeyCode::Tab) | (_, KeyCode::Right) | (_, KeyCode::Char('l')) => {
                        app.tab = app.tab.next();
                        app.scroll = 0;
                    }
                    (KeyModifiers::SHIFT, KeyCode::BackTab)
                    | (_, KeyCode::Left)
                    | (_, KeyCode::Char('h')) => {
                        app.tab = app.tab.prev();
                        app.scroll = 0;
                    }
                    // Number keys 1-5 select tab directly
                    (_, KeyCode::Char('1')) => { app.tab = Tab::Status;    app.scroll = 0; }
                    (_, KeyCode::Char('2')) => { app.tab = Tab::Remaps;    app.scroll = 0; }
                    (_, KeyCode::Char('3')) => { app.tab = Tab::CapsLayer; app.scroll = 0; }
                    (_, KeyCode::Char('4')) => { app.tab = Tab::Config;    app.scroll = 0; }
                    (_, KeyCode::Char('5')) => { app.tab = Tab::Sync;      app.scroll = 0; }

                    // Sync tab — j/k navigate the app list, space/enter toggle, w save
                    (_, KeyCode::Down) | (_, KeyCode::Char('j'))
                        if app.tab == Tab::Sync =>
                    {
                        app.sync.move_down();
                    }
                    (_, KeyCode::Up) | (_, KeyCode::Char('k'))
                        if app.tab == Tab::Sync =>
                    {
                        app.sync.move_up();
                    }
                    (_, KeyCode::Char(' ')) | (_, KeyCode::Enter)
                        if app.tab == Tab::Sync =>
                    {
                        app.sync.toggle_selected();
                    }
                    (_, KeyCode::Char('w')) if app.tab == Tab::Sync => {
                        app.save_sync();
                    }

                    // Scroll (other tabs)
                    (_, KeyCode::Down) | (_, KeyCode::Char('j')) => {
                        app.scroll = app.scroll.saturating_add(1);
                    }
                    (_, KeyCode::Up) | (_, KeyCode::Char('k')) => {
                        app.scroll = app.scroll.saturating_sub(1);
                    }
                    (_, KeyCode::PageDown) => { app.scroll = app.scroll.saturating_add(10); }
                    (_, KeyCode::PageUp)   => { app.scroll = app.scroll.saturating_sub(10); }
                    // Output select (Remaps / CapsLayer tabs)
                    (_, KeyCode::Char('a')) | (_, KeyCode::Char('A')) => {
                        app.output_idx = 0;
                    }
                    (_, KeyCode::Char('b')) | (_, KeyCode::Char('B')) => {
                        app.output_idx = 1;
                    }
                    // Status tab: manual refresh
                    (_, KeyCode::Char('r')) if app.tab == Tab::Status => {
                        app.refresh_status();
                    }
                    // Config tab actions
                    (_, KeyCode::Char('r')) if app.tab == Tab::Config => {
                        app.reload_config();
                    }
                    (_, KeyCode::Char('e')) if app.tab == Tab::Config => {
                        app.open_in_editor();
                        terminal.clear()?;
                    }
                    // Git actions (available from any tab)
                    (_, KeyCode::Char('p')) => {
                        app.git_pull();
                    }
                    (_, KeyCode::Char('u')) => {
                        app.git_push();
                    }
                    _ => {}
                }
            }
        }
    }
}
