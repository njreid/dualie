use anyhow::{Context, Result};
use std::process::Command;

use super::AppEntry;

/// Launch an app by macOS bundle ID using `open -b`.
pub fn launch_app(bundle_id: &str) -> Result<()> {
    Command::new("open")
        .args(["-b", bundle_id])
        .spawn()
        .with_context(|| format!("open -b {bundle_id}"))?;
    Ok(())
}

/// Enumerate installed .app bundles using `system_profiler SPApplicationsDataType`.
/// Falls back to a fast `mdfind` spotlight query if system_profiler is slow.
pub fn list_apps() -> Result<Vec<AppEntry>> {
    // Use mdfind to enumerate .app bundles quickly via Spotlight metadata
    let output = Command::new("mdfind")
        .args(["kMDItemContentType == 'com.apple.application-bundle'"])
        .output()
        .context("mdfind failed – is Spotlight enabled?")?;

    let mut apps: Vec<AppEntry> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|path| parse_app_bundle(path))
        .collect();

    apps.sort_by(|a, b| a.name.cmp(&b.name));
    apps.dedup_by(|a, b| a.id == b.id);
    Ok(apps)
}

fn parse_app_bundle(path: &str) -> Option<AppEntry> {
    let path = path.trim();
    if path.is_empty() || !path.ends_with(".app") {
        return None;
    }

    // Read the bundle ID from the Info.plist using `defaults read`
    let plist = format!("{}/Contents/Info", path);
    let bundle_id = Command::new("defaults")
        .args(["read", &plist, "CFBundleIdentifier"])
        .output()
        .ok()
        .and_then(|o| {
            let s = String::from_utf8_lossy(&o.stdout).trim().to_owned();
            if s.is_empty() { None } else { Some(s) }
        })?;

    let name = Command::new("defaults")
        .args(["read", &plist, "CFBundleDisplayName"])
        .output()
        .ok()
        .and_then(|o| {
            let s = String::from_utf8_lossy(&o.stdout).trim().to_owned();
            if s.is_empty() { None } else { Some(s) }
        })
        .unwrap_or_else(|| {
            // Fall back to the .app filename sans extension
            std::path::Path::new(path)
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default()
        });

    // Icon path (the .icns inside the bundle) – optional, UI can load via img src
    let icon = Command::new("defaults")
        .args(["read", &plist, "CFBundleIconFile"])
        .output()
        .ok()
        .and_then(|o| {
            let icon_file = String::from_utf8_lossy(&o.stdout).trim().to_owned();
            if icon_file.is_empty() { return None; }
            let icon_file = if icon_file.ends_with(".icns") {
                icon_file
            } else {
                format!("{icon_file}.icns")
            };
            let icon_path = format!("{path}/Contents/Resources/{icon_file}");
            if std::path::Path::new(&icon_path).exists() {
                Some(icon_path)
            } else {
                None
            }
        });

    Some(AppEntry { id: bundle_id, name, icon })
}
