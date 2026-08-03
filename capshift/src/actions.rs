//! actions.rs — fire the side effect for a matched Action binding.
//!
//! macOS-only: `open -b <bundle_id>` focuses or launches an app; `sh -c`
//! runs an arbitrary shell command. Both are spawn-and-forget — failures
//! are logged, never propagated, matching a KVM shortcut's expected
//! fire-and-forget behavior. Ported from `daemon/src/launch.rs`'s macOS
//! branch (the Linux gtk-launch/gio branch is not needed here).

use tracing::{info, warn};

use crate::chord::Action;

pub fn fire(action: &Action) {
    match action {
        Action::AppLaunch { app_id, label } => {
            info!(label, app_id, "launching app");
            launch_app(app_id, label);
        }
        Action::ShellCommand { command, label } => {
            info!(label, command, "running shell command");
            run_shell(command, label);
        }
    }
}

// `open -b <bundle_id>` brings the app to the foreground if already running,
// or launches it fresh. `-g` (background) is intentionally omitted so the
// app focuses immediately, which is the expected shortcut behaviour.
#[cfg(target_os = "macos")]
fn launch_app(app_id: &str, label: &str) {
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

#[cfg(not(target_os = "macos"))]
fn launch_app(app_id: &str, label: &str) {
    warn!(label, app_id, "app launching not implemented on this platform");
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fire_shell_command_does_not_panic() {
        // "true" exists on every Unix; this only verifies fire() doesn't
        // panic when dispatching — spawn() is fire-and-forget so we can't
        // (and don't need to) assert on the child process's exit.
        fire(&Action::ShellCommand { command: "true".into(), label: "test".into() });
    }
}
