use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

use super::AppEntry;

/// XDG data directories for .desktop files
fn desktop_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![
        PathBuf::from("/usr/share/applications"),
        PathBuf::from("/usr/local/share/applications"),
    ];

    // XDG_DATA_HOME (usually ~/.local/share/applications)
    if let Some(xdg_home) = std::env::var_os("XDG_DATA_HOME") {
        dirs.push(PathBuf::from(xdg_home).join("applications"));
    } else if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join(".local/share/applications"));
    }

    // XDG_DATA_DIRS (colon-separated extra data dirs)
    if let Ok(xdg_dirs) = std::env::var("XDG_DATA_DIRS") {
        for d in xdg_dirs.split(':') {
            dirs.push(PathBuf::from(d).join("applications"));
        }
    }

    dirs
}

/// Walk all XDG application dirs and parse .desktop files.
pub fn list_apps() -> Result<Vec<AppEntry>> {
    let mut apps = Vec::new();

    for dir in desktop_dirs() {
        if !dir.exists() { continue; }

        let entries = std::fs::read_dir(&dir)
            .with_context(|| format!("reading {}", dir.display()))?;

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("desktop") {
                continue;
            }
            if let Some(app) = parse_desktop_file(&path) {
                apps.push(app);
            }
        }
    }

    apps.sort_by(|a, b| a.name.cmp(&b.name));
    apps.dedup_by(|a, b| a.id == b.id);
    Ok(apps)
}

fn parse_desktop_file(path: &Path) -> Option<AppEntry> {
    let content = std::fs::read_to_string(path).ok()?;
    let mut name: Option<String>    = None;
    let mut no_display = false;
    let mut hidden     = false;
    let mut icon: Option<String>    = None;
    let mut in_section = false;

    for line in content.lines() {
        let line = line.trim();
        if line == "[Desktop Entry]" {
            in_section = true;
            continue;
        }
        if line.starts_with('[') {
            in_section = false;
            continue;
        }
        if !in_section { continue; }

        if let Some(v) = line.strip_prefix("Name=") {
            if name.is_none() { name = Some(v.to_owned()); }
        } else if let Some(v) = line.strip_prefix("NoDisplay=") {
            no_display = v.eq_ignore_ascii_case("true");
        } else if let Some(v) = line.strip_prefix("Hidden=") {
            hidden = v.eq_ignore_ascii_case("true");
        } else if let Some(v) = line.strip_prefix("Icon=") {
            icon = Some(v.to_owned());
        }
    }

    if no_display || hidden { return None; }

    let name = name?;
    // Use the .desktop filename (sans extension) as the stable ID
    let id = path.file_stem()?.to_string_lossy().to_string();

    // Resolve icon path: if it's already an absolute path use it, otherwise
    // delegate to the UI (it can call `xdg-open` or use an icon theme).
    let icon = icon.map(|i| {
        if i.starts_with('/') { i } else { i } // theme resolution left to client
    });

    Some(AppEntry { id, name, icon })
}

/// Launch a .desktop application by its ID (basename of the .desktop file).
pub fn launch_app(desktop_id: &str) -> Result<()> {
    // gtk-launch is the most portable XDG launcher
    if Command::new("gtk-launch").arg(desktop_id).spawn().is_ok() {
        return Ok(());
    }
    // Fallback: find the Exec= line ourselves
    for dir in desktop_dirs() {
        let path = dir.join(format!("{desktop_id}.desktop"));
        if let Some(exec) = read_exec_line(&path) {
            let parts: Vec<&str> = exec.split_whitespace().collect();
            if let Some((cmd, args)) = parts.split_first() {
                Command::new(cmd)
                    .args(args.iter().filter(|a| !a.starts_with('%')))
                    .spawn()
                    .with_context(|| format!("launching {exec}"))?;
                return Ok(());
            }
        }
    }
    Err(anyhow::anyhow!("could not find or launch {desktop_id}"))
}

fn read_exec_line(path: &Path) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    let mut in_section = false;
    for line in content.lines() {
        let line = line.trim();
        if line == "[Desktop Entry]" { in_section = true; continue; }
        if line.starts_with('[') { in_section = false; continue; }
        if in_section {
            if let Some(v) = line.strip_prefix("Exec=") {
                return Some(v.to_owned());
            }
        }
    }
    None
}
