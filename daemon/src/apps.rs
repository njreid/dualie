/// apps.rs — App config registry for cross-machine file sync.
///
/// Two sources are merged at load time:
///   1. `known_apps.kdl`  — embedded in the binary; ships with dualie.
///   2. `user_apps.kdl`   — `~/.config/dualie/user_apps.kdl`; user overrides.
///
/// An entry in `user_apps.kdl` **completely replaces** the matching
/// `known_apps.kdl` entry.  To remove a built-in app from the registry,
/// add an empty entry: `app "tmux" {}`.
///
/// # Format
///
/// ```kdl
/// app "helix" label="Helix" {
///     comment "#"      // line-comment prefix; default "//"
///     unix   "~/.config/helix/config.toml" "~/.config/helix/languages.toml"
///     macos  "~/.config/helix/config.toml"
/// }
/// ```
///
/// Platform keys: `linux`, `macos`, `windows`, `unix` (linux+macos), `all`.
/// Globs are home-relative; `~/` is expanded at resolution time.
/// `comment ""` disables local-block guards (use for formats like JSON).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use kdl::{KdlDocument, KdlNode};
use tracing::warn;

use crate::config::kdl_config_path;

// ── Public types ──────────────────────────────────────────────────────────────

/// The built-in registry, embedded at compile time.
const KNOWN_APPS_KDL: &str = include_str!("../known_apps.kdl");

/// An entry in the app registry.
#[derive(Debug, Clone)]
pub struct AppDef {
    /// Short machine-readable name (e.g. `"zellij"`).
    pub name: String,
    /// Human-readable label (e.g. `"Zellij"`).
    #[allow(dead_code)]
    pub label: String,
    /// Line-comment prefix for this app's config format (e.g. `"#"`, `"//"`).
    /// Empty string means no comment support — local guards are disabled.
    pub comment_char: String,
    /// Per-platform glob lists.  Keys: `linux`, `macos`, `windows`, `unix`, `all`.
    pub platform_globs: HashMap<String, Vec<String>>,
}

impl AppDef {
    /// Return the globs applicable on the current platform.
    pub fn globs_for_current_platform(&self) -> Vec<&str> {
        let platform = current_platform();
        let mut out: Vec<&str> = Vec::new();
        // `all` applies everywhere.
        if let Some(v) = self.platform_globs.get("all") {
            out.extend(v.iter().map(String::as_str));
        }
        // `unix` applies on linux and macos.
        if matches!(platform, "linux" | "macos") {
            if let Some(v) = self.platform_globs.get("unix") {
                out.extend(v.iter().map(String::as_str));
            }
        }
        // Exact platform match.
        if let Some(v) = self.platform_globs.get(platform) {
            out.extend(v.iter().map(String::as_str));
        }
        out
    }

    /// Expand all applicable globs against the current user home directory.
    /// Returns sorted, deduplicated absolute paths that actually exist on disk.
    pub fn expand_globs(&self) -> Vec<PathBuf> {
        let home = match home_dir() {
            Some(h) => h,
            None    => return Vec::new(),
        };
        let mut paths: Vec<PathBuf> = Vec::new();
        for pattern in self.globs_for_current_platform() {
            let expanded = expand_home(pattern, &home);
            match glob::glob(&expanded.to_string_lossy()) {
                Ok(entries) => {
                    for entry in entries.flatten() {
                        if entry.is_file() && !paths.contains(&entry) {
                            paths.push(entry);
                        }
                    }
                }
                Err(e) => warn!("invalid glob {expanded:?}: {e}"),
            }
        }
        paths.sort();
        paths
    }
}

/// The merged app registry (known_apps + user_apps).
#[derive(Debug, Clone, Default)]
pub struct AppRegistry {
    /// Ordered list of app names (insertion order, user apps override built-ins).
    order: Vec<String>,
    apps:  HashMap<String, AppDef>,
}

impl AppRegistry {
    /// Load and merge `known_apps.kdl` + `user_apps.kdl`.
    pub fn load() -> Result<Self> {
        let mut registry = Self::from_kdl(KNOWN_APPS_KDL, "known_apps.kdl")?;

        let user_path = user_apps_path();
        if user_path.exists() {
            let src = std::fs::read_to_string(&user_path)
                .with_context(|| format!("reading {}", user_path.display()))?;
            let user_reg = Self::from_kdl(&src, &user_path.to_string_lossy())?;
            registry.merge_user(user_reg);
        }

        Ok(registry)
    }

    /// Look up an app by name.
    pub fn get(&self, name: &str) -> Option<&AppDef> {
        self.apps.get(name)
    }

    /// Iterate apps in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = &AppDef> {
        self.order.iter().filter_map(|n| self.apps.get(n))
    }

    /// Number of apps in the registry.
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.order.len()
    }

    /// Parse an app registry from a KDL source string.
    pub fn from_kdl(src: &str, source_label: &str) -> Result<Self> {
        let doc = src.parse::<KdlDocument>()
            .map_err(|e| anyhow::anyhow!("{source_label}: {e:?}"))?;

        let mut reg = Self::default();

        for node in doc.nodes() {
            if node.name().value() != "app" {
                warn!("{source_label}: unknown top-level node {:?}", node.name().value());
                continue;
            }

            let app = parse_app_node(node)
                .with_context(|| format!("{source_label}: app {:?}", node.name().value()))?;

            if !reg.order.contains(&app.name) {
                reg.order.push(app.name.clone());
            }
            reg.apps.insert(app.name.clone(), app);
        }

        Ok(reg)
    }

    /// Merge user registry: user entries replace built-in entries of the same name.
    fn merge_user(&mut self, user: AppRegistry) {
        for name in user.order {
            let app = user.apps[&name].clone();
            if !self.order.contains(&name) {
                self.order.push(name.clone());
            }
            self.apps.insert(name, app);
        }
        self.order.sort(); // alphabetical after merge
    }
}

// ── Parsing ───────────────────────────────────────────────────────────────────

fn parse_app_node(node: &KdlNode) -> Result<AppDef> {
    let name = node.entries()
        .iter()
        .find(|e| e.name().is_none())
        .and_then(|e| e.value().as_string())
        .map(str::to_owned)
        .context("app requires a string name as first argument")?;

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
                other => warn!("app {:?}: unknown key {other:?}", name),
            }
        }
    }

    Ok(AppDef { name, label, comment_char, platform_globs })
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
    kdl_config_path().with_file_name("user_apps.kdl")
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r##"
app "helix" label="Helix" {
    comment "#"
    unix  "~/.config/helix/config.toml" "~/.config/helix/languages.toml"
    macos "~/.config/helix/themes/**"
}
app "zellij" label="Zellij" {
    comment "//"
    all   "~/.config/zellij/config.kdl"
}
"##;

    #[test]
    fn parse_basic() {
        let reg = AppRegistry::from_kdl(SAMPLE, "test").unwrap();
        assert_eq!(reg.len(), 2);
        let helix = reg.get("helix").unwrap();
        assert_eq!(helix.label, "Helix");
        assert_eq!(helix.comment_char, "#");
        assert!(helix.platform_globs.get("unix").unwrap().contains(&"~/.config/helix/config.toml".to_owned()));
    }

    #[test]
    fn user_overrides_builtin() {
        let built_in = AppRegistry::from_kdl(SAMPLE, "builtin").unwrap();
        let user_src = r##"app "helix" label="Helix (custom)" { comment "#" linux "~/custom/helix.toml" }"##;
        let user     = AppRegistry::from_kdl(user_src, "user").unwrap();
        let mut reg  = built_in;
        reg.merge_user(user);

        let helix = reg.get("helix").unwrap();
        assert_eq!(helix.label, "Helix (custom)");
        assert_eq!(helix.comment_char, "#");
        // unix globs from built-in are gone — user entry replaced entirely
        assert!(helix.platform_globs.get("unix").is_none());
    }

    #[test]
    fn empty_comment_char_allowed() {
        let src = r#"app "karabiner" label="K" { comment "" macos "~/.config/karabiner/karabiner.json" }"#;
        let reg = AppRegistry::from_kdl(src, "test").unwrap();
        assert_eq!(reg.get("karabiner").unwrap().comment_char, "");
    }

    #[test]
    fn known_apps_kdl_parses() {
        // Sanity check the embedded registry compiles and parses cleanly.
        let reg = AppRegistry::from_kdl(KNOWN_APPS_KDL, "known_apps.kdl").unwrap();
        assert!(reg.len() >= 20, "expected at least 20 built-in apps, got {}", reg.len());
        // Every app has at least one platform glob.
        for app in reg.iter() {
            assert!(
                !app.platform_globs.is_empty(),
                "app {:?} has no platform globs", app.name
            );
        }
    }

    #[test]
    fn expand_home_prefix() {
        let home = PathBuf::from("/home/user");
        assert_eq!(expand_home("~/.config/helix/config.toml", &home),
                   PathBuf::from("/home/user/.config/helix/config.toml"));
        assert_eq!(expand_home("/absolute/path", &home),
                   PathBuf::from("/absolute/path"));
        assert_eq!(expand_home("~", &home), home);
    }

    #[test]
    fn globs_for_current_platform_includes_unix_on_linux_or_macos() {
        let src = r##"app "test" { unix "~/unix.toml" linux "~/linux.toml" macos "~/macos.toml" all "~/all.toml" }"##;
        let reg = AppRegistry::from_kdl(src, "test").unwrap();
        let app = reg.get("test").unwrap();
        let globs = app.globs_for_current_platform();
        // `all` is always included
        assert!(globs.contains(&"~/all.toml"));
        // On linux or macos, `unix` is included
        if cfg!(any(target_os = "linux", target_os = "macos")) {
            assert!(globs.contains(&"~/unix.toml"));
        }
    }
}
