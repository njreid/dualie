//! config.rs — loads and hot-reloads `~/.config/capshift/config.kdl`.

use std::path::PathBuf;

use anyhow::{Context, Result};
use kdl::{KdlDocument, KdlNode};

use crate::chord::{
    Action, Binding, BindingKey, BindingMap, MOD_COMMAND, MOD_CONTROL, MOD_OPTION, MOD_SHIFT,
};
use crate::keycodes::keycode_by_name;

const DEFAULT_CONFIG: &str = r#"// ~/.config/capshift/config.kdl
//
// bind "<key>" app="<bundle-id>" label="<name>"   — focus/launch an app
// bind "<key>" shell="<command>" label="<name>"    — run a shell command
// bind "<key>" key="<target-key-name>"             — remap caps+<key> to another key
// Add mod="shift", "control", "option", or "command" for caps+modifier+key.

// bind "s" app="com.tinyspeck.slackmacgap" label="Slack"
// bind "m" app="com.apple.mail" label="Mail"
// bind "t" shell="open -a Terminal" label="Terminal"

// caps+h/j/k/l as arrow keys
// bind "h" key="left"
// bind "j" key="down"
// bind "k" key="up"
// bind "l" key="right"
// bind "h" mod="shift" key="home"
// bind "l" mod="shift" key="end"
"#;

pub fn config_path() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").context("HOME environment variable not set")?;
    Ok(PathBuf::from(home).join(".config").join("capshift").join("config.kdl"))
}

/// Parse a KDL config document into a src-HID-keycode -> Binding map.
/// Invalid individual `bind` lines are logged and skipped; parsing keeps
/// going so the rest of the file still loads.
pub fn parse_bindings(src: &str) -> Result<BindingMap> {
    let doc: KdlDocument = src.parse().context("parsing config.kdl")?;
    let mut bindings = BindingMap::new();

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
        let modifier_name = prop_str(node, "mod").or_else(|| prop_str(node, "modifier"));
        let Some(modifiers) = parse_modifiers(modifier_name.unwrap_or("")) else {
            tracing::warn!(key_name, modifier = modifier_name, "config: unknown modifier");
            continue;
        };

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

        let binding_key = BindingKey { hid: src_hid, modifiers };
        if bindings.insert(binding_key, binding).is_some() {
            tracing::warn!(key_name, "config: duplicate binding, last one wins");
        }
    }

    Ok(bindings)
}

pub fn load_or_default() -> Result<BindingMap> {
    let path = config_path()?;
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

/// Watch `config.kdl` for content changes using the platform event backend.
/// The containing directory is watched so editor-style atomic replacements
/// of the file continue to produce reload events.
pub fn watch() -> Result<tokio::sync::watch::Receiver<BindingMap>> {
    let path = config_path()?;
    let initial = load_or_default()?;
    let mut last_source = std::fs::read_to_string(&path)
        .with_context(|| format!("reading {}", path.display()))?;
    let (tx, rx) = tokio::sync::watch::channel(initial);

    tokio::task::spawn_blocking(move || {
        use notify::{RecommendedWatcher, RecursiveMode, Watcher};

        let (event_tx, event_rx) = std::sync::mpsc::channel();
        let mut watcher: RecommendedWatcher = match notify::recommended_watcher(event_tx) {
            Ok(watcher) => watcher,
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
            match event_rx.recv() {
                Ok(Ok(_event)) => {}
                Ok(Err(e)) => {
                    tracing::warn!("config watcher: {e}");
                    continue;
                }
                Err(_) => break,
            }

            // FSEvents can emit several notifications for one save. Drain the
            // already queued batch, then use the file contents as the source
            // of truth. This also avoids reloading for unrelated directory events.
            while event_rx.try_recv().is_ok() {}
            let source = match std::fs::read_to_string(&path) {
                Ok(source) => source,
                Err(e) => {
                    tracing::warn!("config reload {}: {e}", path.display());
                    continue;
                }
            };
            if source == last_source {
                continue;
            }
            last_source = source.clone();

            match parse_bindings(&source) {
                Ok(bindings) => {
                    tracing::info!("config reloaded");
                    if tx.send(bindings).is_err() {
                        break;
                    }
                }
                Err(e) => tracing::error!("config reload: {e:#}"),
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

fn parse_modifiers(value: &str) -> Option<u8> {
    let mut result = 0;
    for name in value.split('+').filter(|part| !part.is_empty()) {
        result |= match name.to_ascii_lowercase().as_str() {
            "shift" => MOD_SHIFT,
            "control" | "ctrl" => MOD_CONTROL,
            "option" | "alt" => MOD_OPTION,
            "command" | "cmd" | "meta" => MOD_COMMAND,
            _ => return None,
        };
    }
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(hid: u8) -> BindingKey {
        BindingKey { hid, modifiers: 0 }
    }

    #[test]
    fn parses_app_binding() {
        let bindings = parse_bindings(r#"bind "s" app="com.tinyspeck.slackmacgap" label="Slack""#).unwrap();
        assert_eq!(
            bindings.get(&key(0x16)), // 's'
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
            bindings.get(&key(0x17)), // 't'
            Some(&Binding::Action(Action::ShellCommand {
                command: "open -a Terminal".into(),
                label: "Terminal".into(),
            }))
        );
    }

    #[test]
    fn parses_key_remap_binding() {
        let bindings = parse_bindings(r#"bind "h" key="left""#).unwrap();
        assert_eq!(bindings.get(&key(0x0B)), Some(&Binding::Remap(0x50))); // 'h' -> left
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
        assert_eq!(bindings.get(&key(0x0B)), Some(&Binding::Remap(0x4F))); // right
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

    #[test]
    fn plain_and_shift_bindings_can_coexist() {
        let bindings = parse_bindings(
            r#"
            bind "h" key="left"
            bind "h" mod="shift" key="home"
            bind "l" modifier="shift" key="end"
            "#,
        )
        .unwrap();
        assert_eq!(bindings.get(&key(0x0B)), Some(&Binding::Remap(0x50)));
        assert_eq!(
            bindings.get(&BindingKey { hid: 0x0B, modifiers: MOD_SHIFT }),
            Some(&Binding::Remap(0x4A))
        );
        assert_eq!(
            bindings.get(&BindingKey { hid: 0x0F, modifiers: MOD_SHIFT }),
            Some(&Binding::Remap(0x4D))
        );
    }
}
