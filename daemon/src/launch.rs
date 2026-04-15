/// launch.rs — Virtual-action dispatch: app launching and shell commands.
///
/// Called from two places:
///   1. `intercept/mod.rs` — when a locally-attached keyboard fires a caps-layer
///      `action` binding.
///   2. `peer.rs` — when the RP2040 sends a `VirtualAction { slot }` message
///      over CDC-ACM (triggered by hardware-connected keyboards or front-panel
///      buttons on the physical switch).
///
/// Platform implementations:
///   Linux  — `gtk-launch <app_id>` (Wayland and X11); falls back to
///             `gio launch <path-to-desktop-file>` if gtk-launch is absent.
///   macOS  — `open -b <bundle_id>`
///   Shell  — `sh -c <command>` (both platforms)

use crate::config::VirtualAction;
use tracing::{info, warn};

/// Fire a virtual action: launch an app or run a shell command.
///
/// For app launches: if the app is already running it is brought to the
/// foreground (focused).  If not running it is started.  This matches the
/// expected behaviour for a KVM shortcut — pressing caps+S on any machine
/// always gives you Slack, regardless of whether it was open.
///
/// Spawns a detached child process and returns immediately.
/// Errors (e.g. app not found) are logged as warnings, not propagated.
pub fn fire(action: &VirtualAction) {
    match action {
        VirtualAction::AppLaunch { app_id, label } => {
            info!(label, app_id, "launching app");
            launch_app(app_id, label);
        }
        VirtualAction::ShellCommand { command, label } => {
            info!(label, command, "running shell command");
            run_shell(command, label);
        }
        VirtualAction::Unset => {}
    }
}

// ── Platform: Linux ───────────────────────────────────────────────────────────

#[cfg(target_os = "linux")]
fn launch_app(app_id: &str, label: &str) {
    // gtk-launch handles both Wayland and X11, and resolves the .desktop ID
    // by searching XDG_DATA_DIRS automatically.
    let result = std::process::Command::new("gtk-launch")
        .arg(app_id)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();

    match result {
        Ok(_) => return,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // gtk-launch not available; try gio launch instead.
        }
        Err(e) => {
            warn!(label, app_id, "gtk-launch: {e}");
            return;
        }
    }

    // Fallback: find the .desktop file and use `gio launch <path>`.
    let desktop_file = find_desktop_file(app_id);
    match desktop_file {
        Some(path) => {
            if let Err(e) = std::process::Command::new("gio")
                .args(["launch", path.to_str().unwrap_or(app_id)])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
            {
                warn!(label, app_id, "gio launch: {e}");
            }
        }
        None => {
            warn!(label, app_id, "app not found: no gtk-launch and no .desktop file located");
        }
    }
}

/// Search standard XDG directories for `<app_id>.desktop`.
#[cfg(target_os = "linux")]
fn find_desktop_file(app_id: &str) -> Option<std::path::PathBuf> {
    let filename = format!("{app_id}.desktop");

    let mut search_dirs: Vec<std::path::PathBuf> = vec![
        "/usr/share/applications".into(),
        "/usr/local/share/applications".into(),
        "/var/lib/snapd/desktop/applications".into(),
        "/var/lib/flatpak/exports/share/applications".into(),
    ];

    if let Some(home) = std::env::var_os("HOME") {
        search_dirs.push(std::path::Path::new(&home).join(".local/share/applications"));
    }
    if let Ok(xdg) = std::env::var("XDG_DATA_DIRS") {
        for dir in xdg.split(':') {
            search_dirs.push(std::path::Path::new(dir).join("applications"));
        }
    }

    for dir in search_dirs {
        let candidate = dir.join(&filename);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

// ── Platform: macOS ───────────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
fn launch_app(app_id: &str, label: &str) {
    // `open -b <bundle_id>` brings the app to the foreground if already running,
    // or launches it fresh.  `-g` (background) is intentionally omitted so the
    // app focuses immediately, which is the expected KVM shortcut behaviour.
    if let Err(e) = std::process::Command::new("open")
        .args(["-b", app_id])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        warn!(label, app_id, "open -b: {e}");
    }
}

// ── Unsupported platforms ─────────────────────────────────────────────────────

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn launch_app(app_id: &str, label: &str) {
    warn!(label, app_id, "app launching not implemented on this platform");
}

// ── Shell command (all platforms) ────────────────────────────────────────────

fn run_shell(command: &str, label: &str) {
    if let Err(e) = std::process::Command::new("sh")
        .args(["-c", command])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        warn!(label, command, "shell command: {e}");
    }
}
