//! config.rs — loads and hot-reloads `~/.config/capshift/config.kdl`.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use kdl::{KdlDocument, KdlNode};

use crate::chord::{Action, Binding};
use crate::keycodes::keycode_by_name;

const DEFAULT_CONFIG: &str = r#"// ~/.config/capshift/config.kdl
//
// bind "<key>" app="<bundle-id>" label="<name>"   — focus/launch an app
// bind "<key>" shell="<command>" label="<name>"    — run a shell command
// bind "<key>" key="<target-key-name>"             — remap caps+<key> to another key

// bind "s" app="com.tinyspeck.slackmacgap" label="Slack"
// bind "m" app="com.apple.mail" label="Mail"
// bind "t" shell="open -a Terminal" label="Terminal"

// caps+h/j/k/l as arrow keys
// bind "h" key="left"
// bind "j" key="down"
// bind "k" key="up"
// bind "l" key="right"
"#;

pub fn config_path() -> PathBuf {
    let home = std::env::var_os("HOME").expect("HOME environment variable not set");
    PathBuf::from(home).join(".config").join("capshift").join("config.kdl")
}

/// Parse a KDL config document into a src-HID-keycode -> Binding map.
/// Invalid individual `bind` lines are logged and skipped; parsing keeps
/// going so the rest of the file still loads.
pub fn parse_bindings(src: &str) -> Result<HashMap<u8, Binding>> {
    let doc: KdlDocument = src.parse().context("parsing config.kdl")?;
    let mut bindings = HashMap::new();

    for node in doc.nodes() {
        if node.name().value() != "bind" {
            tracing::warn!("config: skipping unknown node {:?}", node.name().value());
            continue;
        }

        let Some(key_name) = arg_str(node, 0) else {
            tracing::warn!("config: bind requires a key name as its first argument");
            continue;
        };
        let Some(src_hid) = keycode_by_name(key_name) else {
            tracing::warn!(key_name, "config: unknown key name");
            continue;
        };

        let app = prop_str(node, "app");
        let shell = prop_str(node, "shell");
        let key = prop_str(node, "key");
        let label = prop_str(node, "label");

        let present = [app.is_some(), shell.is_some(), key.is_some()]
            .iter()
            .filter(|p| **p)
            .count();
        if present != 1 {
            tracing::warn!(key_name, "config: bind requires exactly one of app=, shell=, key=");
            continue;
        }

        let binding = if let Some(app_id) = app {
            let Some(label) = label else {
                tracing::warn!(key_name, "config: app= binding requires label=");
                continue;
            };
            Binding::Action(Action::AppLaunch { app_id: app_id.to_string(), label: label.to_string() })
        } else if let Some(command) = shell {
            let Some(label) = label else {
                tracing::warn!(key_name, "config: shell= binding requires label=");
                continue;
            };
            Binding::Action(Action::ShellCommand { command: command.to_string(), label: label.to_string() })
        } else {
            let target_name = key.unwrap();
            let Some(target_hid) = keycode_by_name(target_name) else {
                tracing::warn!(key_name, target_name, "config: unknown target key name");
                continue;
            };
            Binding::Remap(target_hid)
        };

        if bindings.insert(src_hid, binding).is_some() {
            tracing::warn!(key_name, "config: duplicate binding, last one wins");
        }
    }

    Ok(bindings)
}

pub fn load_or_default() -> Result<HashMap<u8, Binding>> {
    let path = config_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("creating config dir {}", dir.display()))?;
    }
    if !path.exists() {
        std::fs::write(&path, DEFAULT_CONFIG)
            .with_context(|| format!("writing default config to {}", path.display()))?;
        tracing::info!("created default config at {}", path.display());
    }
    let src = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    parse_bindings(&src)
}

/// Spawn a file watcher on `config.kdl`. Returns a `watch::Receiver` that
/// yields the latest parsed bindings whenever the file changes.
pub fn watch() -> Result<tokio::sync::watch::Receiver<HashMap<u8, Binding>>> {
    let path = config_path();
    let initial = load_or_default()?;
    let (tx, rx) = tokio::sync::watch::channel(initial);

    tokio::task::spawn_blocking(move || {
        use notify::{RecommendedWatcher, RecursiveMode, Watcher};
        let (ftx, frx) = std::sync::mpsc::channel();
        let mut watcher: RecommendedWatcher = match notify::recommended_watcher(ftx) {
            Ok(w) => w,
            Err(e) => {
                tracing::error!("config watcher: {e}");
                return;
            }
        };

        let watch_dir = path.parent().unwrap_or(&path);
        if let Err(e) = watcher.watch(watch_dir, RecursiveMode::NonRecursive) {
            tracing::error!("watch {}: {e}", watch_dir.display());
            return;
        }

        tracing::info!("watching {} for config changes", path.display());

        loop {
            match frx.recv() {
                Ok(Ok(event)) => {
                    if !event.paths.iter().any(|p| p == &path) {
                        continue;
                    }
                    if matches!(
                        event.kind,
                        notify::EventKind::Create(_)
                            | notify::EventKind::Modify(notify::event::ModifyKind::Data(_))
                            | notify::EventKind::Modify(notify::event::ModifyKind::Any)
                    ) {
                        match load_or_default() {
                            Ok(bindings) => {
                                tracing::info!("config reloaded");
                                let _ = tx.send(bindings);
                            }
                            Err(e) => tracing::error!("config reload: {e:#}"),
                        }
                    }
                }
                Ok(Err(e)) => tracing::warn!("watcher: {e}"),
                Err(_) => break,
            }
        }
    });

    Ok(rx)
}

fn arg_str<'a>(node: &'a KdlNode, idx: usize) -> Option<&'a str> {
    node.entries()
        .iter()
        .filter(|e| e.name().is_none())
        .nth(idx)
        .and_then(|e| e.value().as_string())
}

fn prop_str<'a>(node: &'a KdlNode, key: &str) -> Option<&'a str> {
    node.entries()
        .iter()
        .find(|e| e.name().is_some_and(|n| n.value() == key))
        .and_then(|e| e.value().as_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_app_binding() {
        let bindings = parse_bindings(r#"bind "s" app="com.tinyspeck.slackmacgap" label="Slack""#).unwrap();
        assert_eq!(
            bindings.get(&0x16), // 's'
            Some(&Binding::Action(Action::AppLaunch {
                app_id: "com.tinyspeck.slackmacgap".into(),
                label: "Slack".into(),
            }))
        );
    }

    #[test]
    fn parses_shell_binding() {
        let bindings = parse_bindings(r#"bind "t" shell="open -a Terminal" label="Terminal""#).unwrap();
        assert_eq!(
            bindings.get(&0x17), // 't'
            Some(&Binding::Action(Action::ShellCommand {
                command: "open -a Terminal".into(),
                label: "Terminal".into(),
            }))
        );
    }

    #[test]
    fn parses_key_remap_binding() {
        let bindings = parse_bindings(r#"bind "h" key="left""#).unwrap();
        assert_eq!(bindings.get(&0x0B), Some(&Binding::Remap(0x50))); // 'h' -> left
    }

    #[test]
    fn skips_binding_missing_label() {
        let bindings = parse_bindings(r#"bind "s" app="com.tinyspeck.slackmacgap""#).unwrap();
        assert!(bindings.is_empty());
    }

    #[test]
    fn skips_binding_with_both_app_and_shell() {
        let bindings = parse_bindings(
            r#"bind "s" app="com.tinyspeck.slackmacgap" shell="true" label="Slack""#,
        )
        .unwrap();
        assert!(bindings.is_empty());
    }

    #[test]
    fn skips_binding_with_none_of_app_shell_key() {
        let bindings = parse_bindings(r#"bind "s" label="Slack""#).unwrap();
        assert!(bindings.is_empty());
    }

    #[test]
    fn skips_unknown_source_key_name() {
        let bindings = parse_bindings(r#"bind "notakey" app="com.example.app" label="X""#).unwrap();
        assert!(bindings.is_empty());
    }

    #[test]
    fn skips_unknown_target_key_name() {
        let bindings = parse_bindings(r#"bind "h" key="notakey""#).unwrap();
        assert!(bindings.is_empty());
    }

    #[test]
    fn duplicate_binding_last_one_wins() {
        let bindings = parse_bindings(
            r#"
            bind "h" key="left"
            bind "h" key="right"
            "#,
        )
        .unwrap();
        assert_eq!(bindings.get(&0x0B), Some(&Binding::Remap(0x4F))); // right
    }

    #[test]
    fn unknown_top_level_node_is_skipped_not_fatal() {
        let bindings = parse_bindings(
            r#"
            something-else "foo"
            bind "h" key="left"
            "#,
        )
        .unwrap();
        assert_eq!(bindings.len(), 1);
    }
}
