/// app_registry.rs — App registry reader for the TUI sync tab.
///
/// Loads the same `known_apps.kdl` embedded in the daemon, then merges
/// `user_apps.kdl` on top, using identical logic to daemon/src/apps.rs.
/// Kept as a self-contained copy so the TUI crate has no daemon dependency.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use kdl::{KdlDocument, KdlNode};

// Embed the same registry the daemon uses.
const KNOWN_APPS_KDL: &str = include_str!("../../daemon/known_apps.kdl");

// ── Public types ──────────────────────────────────────────────────────────────

/// One entry in the app registry, as seen by the TUI.
#[derive(Debug, Clone)]
pub struct AppEntry {
    pub name:         String,
    pub label:        String,
    pub comment_char: String,
    /// Raw glob patterns for the current platform (not yet expanded).
    pub globs:        Vec<String>,
}

impl AppEntry {
    /// Expand globs against the current home directory.
    /// Returns only patterns — actual file existence is checked by caller if needed.
    pub fn expand_globs(&self) -> Vec<PathBuf> {
        let home = match home_dir() {
            Some(h) => h,
            None    => return Vec::new(),
        };
        let mut paths: Vec<PathBuf> = Vec::new();
        for pattern in &self.globs {
            let expanded = expand_home(pattern, &home);
            if let Ok(entries) = glob::glob(&expanded.to_string_lossy()) {
                for entry in entries.flatten() {
                    if entry.is_file() && !paths.contains(&entry) {
                        paths.push(entry);
                    }
                }
            }
        }
        paths.sort();
        paths
    }
}

// ── Registry loader ───────────────────────────────────────────────────────────

/// Load the merged registry: `known_apps.kdl` (embedded) + `user_apps.kdl`.
/// Returns apps in alphabetical order; user entries replace built-in entries.
pub fn load_registry() -> Vec<AppEntry> {
    let mut map: HashMap<String, AppEntry> = HashMap::new();
    let mut order: Vec<String> = Vec::new();

    // Built-in apps.
    if let Ok(doc) = KNOWN_APPS_KDL.parse::<KdlDocument>() {
        for node in doc.nodes() {
            if let Some(entry) = parse_entry(node) {
                if !order.contains(&entry.name) {
                    order.push(entry.name.clone());
                }
                map.insert(entry.name.clone(), entry);
            }
        }
    }

    // User overrides.
    let user_path = user_apps_path();
    if user_path.exists() {
        if let Ok(src) = std::fs::read_to_string(&user_path) {
            if let Ok(doc) = src.parse::<KdlDocument>() {
                for node in doc.nodes() {
                    if let Some(entry) = parse_entry(node) {
                        if !order.contains(&entry.name) {
                            order.push(entry.name.clone());
                        }
                        map.insert(entry.name.clone(), entry);
                    }
                }
            }
        }
    }

    order.sort();
    order.into_iter().filter_map(|n| map.remove(&n)).collect()
}

// ── Parsing ───────────────────────────────────────────────────────────────────

fn parse_entry(node: &KdlNode) -> Option<AppEntry> {
    if node.name().value() != "app" {
        return None;
    }

    let name = node.entries()
        .iter()
        .find(|e| e.name().is_none())
        .and_then(|e| e.value().as_string())
        .map(str::to_owned)?;

    let label = node.entries()
        .iter()
        .find(|e| e.name().is_some_and(|n| n.value() == "label"))
        .and_then(|e| e.value().as_string())
        .unwrap_or(&name)
        .to_owned();

    let mut comment_char = "//".to_owned();
    let mut platform_globs: HashMap<String, Vec<String>> = HashMap::new();

    if let Some(children) = node.children() {
        for child in children.nodes() {
            match child.name().value() {
                "comment" => {
                    comment_char = child.entries()
                        .iter()
                        .find(|e| e.name().is_none())
                        .and_then(|e| e.value().as_string())
                        .unwrap_or("")
                        .to_owned();
                }
                platform @ ("linux" | "macos" | "windows" | "unix" | "all") => {
                    let globs: Vec<String> = child.entries()
                        .iter()
                        .filter(|e| e.name().is_none())
                        .filter_map(|e| e.value().as_string())
                        .map(str::to_owned)
                        .collect();
                    platform_globs.entry(platform.to_owned())
                        .or_default()
                        .extend(globs);
                }
                _ => {}
            }
        }
    }

    let globs = globs_for_current_platform(&platform_globs);
    Some(AppEntry { name, label, comment_char, globs })
}

fn globs_for_current_platform(platform_globs: &HashMap<String, Vec<String>>) -> Vec<String> {
    let platform = current_platform();
    let mut out = Vec::new();
    if let Some(v) = platform_globs.get("all") {
        out.extend(v.iter().cloned());
    }
    if matches!(platform, "linux" | "macos") {
        if let Some(v) = platform_globs.get("unix") {
            out.extend(v.iter().cloned());
        }
    }
    if let Some(v) = platform_globs.get(platform) {
        out.extend(v.iter().cloned());
    }
    out
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn current_platform() -> &'static str {
    #[cfg(target_os = "linux")]   { "linux" }
    #[cfg(target_os = "macos")]   { "macos" }
    #[cfg(target_os = "windows")] { "windows" }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    { "unknown" }
}

fn home_dir() -> Option<PathBuf> {
    directories::UserDirs::new().map(|d| d.home_dir().to_path_buf())
}

fn expand_home(pattern: &str, home: &Path) -> PathBuf {
    if let Some(rest) = pattern.strip_prefix("~/") {
        home.join(rest)
    } else if pattern == "~" {
        home.to_path_buf()
    } else {
        PathBuf::from(pattern)
    }
}

pub fn user_apps_path() -> PathBuf {
    if let Some(proj) = directories::ProjectDirs::from("", "", "dualie") {
        proj.config_dir().join("user_apps.kdl")
    } else {
        PathBuf::from("user_apps.kdl")
    }
}
