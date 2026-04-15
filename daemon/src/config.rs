use anyhow::{bail, Context, Result};
use directories::ProjectDirs;
use kdl::{KdlDocument, KdlNode, KdlValue};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;


// ── Constants ─────────────────────────────────────────────────────────────────

/// Number of virtual action slots (0-31).
pub const DUALIE_VKEY_COUNT: usize = 32;

/// Starter config written on first run when no `dualie.kdl` exists.
const DEFAULT_CONFIG: &str = r#"// dualie.kdl — Dualie configuration
// Docs: https://github.com/njreid/dualie
//
// ports         — map physical output ports (a/b) to machine names
// machine <n>   — per-machine key remaps, caps layer, and sync skip list
// sync          — apps whose config files to sync between machines
// git-sync      — remote git repo for config versioning

ports {
    a desk
    b laptop
}

machine desk {
    // remap {
    //     key capslock esc          // remap a key
    //     modifier lalt lctrl       // swap modifiers
    // }

    layers {
        caps {
            // chord h left          // caps+H → Left arrow
            // chord l right         // caps+L → Right arrow
            // chord k up
            // chord j down
            // swap  n               // caps+N → switch to other output
        }
    }

    // skip {
    //     app "hammerspoon"         // don't sync this app to this machine
    // }
}

machine laptop {
}

sync {
    // app "fish"
    // app "neovim"
    // app "git"
}

// git-sync {
//     remote "git@github.com:you/dotfiles.git"
// }
"#;

// ── Virtual action definitions ────────────────────────────────────────────────

/// The type of action the daemon should perform when a virtual key fires.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum VirtualAction {
    /// Launch or focus an application by its platform ID.
    AppLaunch {
        /// macOS: bundle ID ("com.tinyspeck.slackmacgap")
        /// Linux: .desktop basename ("slack")
        app_id: String,
        label:  String,
    },
    /// Run a shell command
    ShellCommand {
        command: String,
        label:   String,
    },
    /// Placeholder / unassigned slot
    Unset,
}

impl VirtualAction {
    pub fn label(&self) -> Option<&str> {
        match self {
            Self::AppLaunch  { label, .. } => Some(label),
            Self::ShellCommand { label, .. } => Some(label),
            Self::Unset => None,
        }
    }
}

impl Default for VirtualAction {
    fn default() -> Self { Self::Unset }
}

// ── Modifier remaps ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModifierRemap {
    pub src: u8,
    pub dst: u8,
}

// ── Key remaps ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct KeyRemap {
    pub src_keycode:  u8,
    pub dst_keycode:  u8,
    #[serde(default)]
    pub src_modifier: u8,
    #[serde(default)]
    pub dst_modifier: u8,
    #[serde(default = "default_output_mask")]
    pub output_mask:  u8,
    #[serde(default)]
    pub flags:        u8,
}

fn default_output_mask() -> u8 { 3 }

// ── Caps layer ────────────────────────────────────────────────────────────────

pub const CAPS_LAYER_MAX: usize = 32;

pub const CAPS_ENTRY_CHORD:     u8 = 0;
pub const CAPS_ENTRY_VIRTUAL:   u8 = 1;
#[allow(dead_code)] pub const CAPS_ENTRY_JUMP_A:     u8 = 2;
#[allow(dead_code)] pub const CAPS_ENTRY_JUMP_B:     u8 = 3;
#[allow(dead_code)] pub const CAPS_ENTRY_SWAP:       u8 = 4;
#[allow(dead_code)] pub const CAPS_ENTRY_CLIP_PULL:  u8 = 5;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapsLayerEntry {
    pub src_keycode:  u8,
    #[serde(default)]
    pub entry_type:   u8,
    #[serde(default = "default_output_mask")]
    pub output_mask:  u8,
    #[serde(default)]
    pub dst_modifier: u8,
    #[serde(default)]
    pub dst_keycodes: [u8; 4],
    #[serde(default)]
    pub vaction_idx:  u8,
}

impl Default for CapsLayerEntry {
    fn default() -> Self {
        Self {
            src_keycode:  0,
            entry_type:   CAPS_ENTRY_CHORD,
            output_mask:  3,
            dst_modifier: 0,
            dst_keycodes: [0; 4],
            vaction_idx:  0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapsLayer {
    #[serde(default = "default_true")]
    pub unmapped_passthrough: bool,
    #[serde(default)]
    pub entries: Vec<CapsLayerEntry>,
}

fn default_true() -> bool { true }

impl Default for CapsLayer {
    fn default() -> Self {
        Self { unmapped_passthrough: true, entries: Vec::new() }
    }
}

// ── Per-machine config ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MachineConfig {
    #[serde(default = "default_actions")]
    pub virtual_actions: Vec<VirtualAction>,
    #[serde(default)]
    pub key_remaps: Vec<KeyRemap>,
    #[serde(default)]
    pub modifier_remaps: Vec<ModifierRemap>,
    #[serde(default)]
    pub caps_layer: CapsLayer,
    /// Apps that should NOT be synced to this machine (still stored in git and
    /// synced to other machines).
    #[serde(default)]
    pub skip: Vec<String>,
}

fn default_actions() -> Vec<VirtualAction> {
    vec![VirtualAction::Unset; DUALIE_VKEY_COUNT]
}

impl Default for MachineConfig {
    fn default() -> Self {
        Self {
            virtual_actions: default_actions(),
            key_remaps:      Vec::new(),
            modifier_remaps: Vec::new(),
            caps_layer:      CapsLayer::default(),
            skip:            Vec::new(),
        }
    }
}

/// Backwards-compat type alias so existing internal call sites still compile
/// while we migrate.
#[allow(dead_code)]
pub type OutputDaemonConfig = MachineConfig;

// ── Sync config ───────────────────────────────────────────────────────────────

/// Which app configs and manual file pairs to sync across machines.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct SyncConfig {
    /// Registry app names explicitly added by the user via the TUI.
    /// Each name is looked up in the app registry at sync time.
    #[serde(default)]
    pub apps: Vec<String>,
    /// Manual file pairs: `pair "~/.tmux.conf" "~/.tmux.conf"`.
    #[serde(default)]
    pub pairs: Vec<ManualSyncPair>,
}

/// A manually specified file pair for syncing.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ManualSyncPair {
    pub local:  String,
    pub remote: String,
}

impl SyncConfig {
    /// Add an app name if not already present.
    pub fn add_app(&mut self, name: &str) {
        if !self.apps.iter().any(|a| a == name) {
            self.apps.push(name.to_owned());
        }
    }

    /// Remove an app name; returns true if it was present.
    #[allow(dead_code)]
    pub fn remove_app(&mut self, name: &str) -> bool {
        let before = self.apps.len();
        self.apps.retain(|a| a != name);
        self.apps.len() < before
    }
}

// ── Git sync config ───────────────────────────────────────────────────────────

/// Git-backed config sync settings (stored in `dualie.kdl`, shared across machines).
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct GitSyncConfig {
    /// Remote URL, e.g. `git@github.com:user/configs.git`.
    #[serde(default)]
    pub remote: Option<String>,
}

// ── Top-level daemon config ───────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DualieConfig {
    /// Maps port index (0=A, 1=B) to a machine name.
    #[serde(default)]
    pub ports: [Option<String>; 2],
    /// Per-machine configs, keyed by machine name.
    #[serde(default)]
    pub machines: HashMap<String, MachineConfig>,
    #[serde(default)]
    pub sync: SyncConfig,
    #[serde(default)]
    pub git_sync: GitSyncConfig,
}

impl Default for DualieConfig {
    fn default() -> Self {
        Self {
            ports:    [None, None],
            machines: HashMap::new(),
            sync:     SyncConfig::default(),
            git_sync: GitSyncConfig::default(),
        }
    }
}

impl DualieConfig {
    /// Resolve port index (0=A, 1=B) to the machine config for that port.
    /// Returns `None` if the port has no machine assigned or the machine name
    /// isn't found in `machines`.
    pub fn resolve_port(&self, port_idx: usize) -> Option<&MachineConfig> {
        self.ports.get(port_idx)
            .and_then(|opt| opt.as_deref())
            .and_then(|name| self.machines.get(name))
    }

    /// Returns `true` if `machine_name` has `app_name` in its `skip` list.
    pub fn machine_skips(&self, machine_name: &str, app_name: &str) -> bool {
        self.machines.get(machine_name)
            .map(|m| m.skip.iter().any(|s| s == app_name))
            .unwrap_or(false)
    }

    // ── I/O ──────────────────────────────────────────────────────────────────

    /// Load config: try `dualie.kdl`, then legacy `config.json`, then default.
    pub fn load_or_default() -> Result<Self> {
        let kdl_path = kdl_config_path();
        if kdl_path.exists() {
            let src = std::fs::read_to_string(&kdl_path)
                .with_context(|| format!("reading {}", kdl_path.display()))?;
            return Self::from_kdl(&src)
                .with_context(|| format!("parsing {}", kdl_path.display()));
        }

        let json_path = json_config_path();
        if json_path.exists() {
            tracing::info!("loading legacy config from {}", json_path.display());
            let raw = std::fs::read_to_string(&json_path)
                .with_context(|| format!("reading {}", json_path.display()))?;
            return serde_json::from_str::<Self>(&raw)
                .with_context(|| format!("parsing {}", json_path.display()));
        }

        Ok(Self::default())
    }

    /// Save config to `dualie.kdl`.
    #[allow(dead_code)]
    pub fn save(&self) -> Result<()> {
        let path = kdl_config_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, self.to_kdl_string())
            .with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    }

    /// Export config as a CBOR blob (for firmware push).
    #[allow(dead_code)]
    pub fn to_cbor(&self) -> Result<Vec<u8>> {
        let mut buf = Vec::new();
        ciborium::into_writer(self, &mut buf)?;
        Ok(buf)
    }

    /// Import config from a CBOR blob.
    #[allow(dead_code)]
    pub fn from_cbor(bytes: &[u8]) -> Result<Self> {
        Ok(ciborium::from_reader(bytes)?)
    }

    // ── KDL parsing ──────────────────────────────────────────────────────────

    /// Parse a `DualieConfig` from a KDL source string.
    ///
    /// Format:
    /// ```text
    /// output A {
    ///     actions {
    ///         launch "Slack" app-id="com.tinyspeck.slackmacgap"
    ///         shell  "Terminal" command="open -a Terminal"
    ///     }
    ///
    ///     remap {
    ///         key a left           // single char = qwerty, named = known key list
    ///         key 0x39 0x29        // hex/decimal = raw HID keycode
    ///         modifier lalt rctrl  // src-mod dst-mod (named modifiers)
    ///     }
    ///
    ///     layers {
    ///         caps {
    ///             chord  a e            // caps+A → E
    ///             chord  b lctrl t      // caps+B → Ctrl+T
    ///             action s Slack        // caps+S → fire Slack action
    ///             jump-a h              // caps+H → switch to output A
    ///             jump-b k              // caps+K → switch to output B
    ///             swap   n              // caps+N → toggle output
    ///         }
    ///     }
    /// }
    ///
    /// output B { }
    /// ```
    pub fn from_kdl(src: &str) -> Result<Self> {
        let doc = src.parse::<KdlDocument>()
            .map_err(|e| anyhow::anyhow!("{:?}",
                miette::Report::new(e).with_source_code(src.to_owned())))?;

        let mut cfg = Self::default();

        for node in doc.nodes() {
            match node.name().value() {
                "ports" => {
                    if let Some(children) = node.children() {
                        for child in children.nodes() {
                            let port = child.name().value();
                            let machine = kdl_arg_str(child, 0)
                                .ok_or_else(|| anyhow::anyhow!("ports.{port}: requires a machine name argument"))?;
                            match port {
                                "a" | "A" => cfg.ports[0] = Some(machine.to_owned()),
                                "b" | "B" => cfg.ports[1] = Some(machine.to_owned()),
                                other => bail!("ports: unknown port {other:?}; expected a or b"),
                            }
                        }
                    }
                }
                "machine" => {
                    let name = kdl_arg_str(node, 0)
                        .ok_or_else(|| anyhow::anyhow!("machine requires a name argument"))?
                        .to_owned();
                    let mut mc = MachineConfig::default();
                    if let Some(children) = node.children() {
                        parse_machine(children, &mut mc)?;
                    }
                    cfg.machines.insert(name, mc);
                }
                "sync" => {
                    if let Some(children) = node.children() {
                        parse_sync(children, &mut cfg.sync)?;
                    }
                }
                "git-sync" => {
                    if let Some(children) = node.children() {
                        for child in children.nodes() {
                            if child.name().value() == "remote" {
                                if let Some(s) = kdl_arg_str(child, 0) {
                                    cfg.git_sync.remote = Some(s.to_owned());
                                }
                            }
                        }
                    }
                }
                other => tracing::warn!("unknown top-level node: {other}"),
            }
        }

        // Validate: each port that names a machine must exist in machines.
        for (port_label, port_name) in [("a", &cfg.ports[0]), ("b", &cfg.ports[1])] {
            if let Some(name) = port_name {
                if !cfg.machines.contains_key(name.as_str()) {
                    bail!(
                        "ports.{port_label}: machine {name:?} not defined; \
                         add `machine {name} {{ }}` to dualie.kdl"
                    );
                }
            }
        }

        Ok(cfg)
    }

    // ── KDL serialisation ─────────────────────────────────────────────────────

    /// Serialise config to a KDL string.
    #[allow(dead_code)]
    pub fn to_kdl_string(&self) -> String {
        let mut s = String::from(
            "// dualie.kdl\n\
             // Keys: single char (a-z, 0-9), named (esc left volup …), or 0x hex.\n\
             // Modifiers: lctrl lshift lalt lmeta rctrl rshift ralt rmeta\n\n"
        );

        // ports block
        let has_ports = self.ports.iter().any(|p| p.is_some());
        if has_ports {
            s.push_str("ports {\n");
            for (i, port) in self.ports.iter().enumerate() {
                let label = if i == 0 { "a" } else { "b" };
                if let Some(name) = port {
                    s.push_str(&format!("    {label} {name}\n"));
                }
            }
            s.push_str("}\n\n");
        }

        // machine blocks — iterate in insertion order (HashMap is non-deterministic;
        // sort for stable output)
        let mut machine_names: Vec<&str> = self.machines.keys().map(|k| k.as_str()).collect();
        machine_names.sort();

        for name in machine_names {
            let out = &self.machines[name];
            s.push_str(&format!("machine {name} {{\n"));

            // actions block (non-Unset only)
            let non_unset: Vec<(usize, &VirtualAction)> = out.virtual_actions.iter()
                .enumerate()
                .filter(|(_, a)| !matches!(a, VirtualAction::Unset))
                .collect();
            if !non_unset.is_empty() {
                s.push_str("    actions {\n");
                for (_, action) in &non_unset {
                    match action {
                        VirtualAction::AppLaunch { label, app_id } => {
                            s.push_str(&format!(
                                "        launch {label:?} app-id={app_id:?}\n"
                            ));
                        }
                        VirtualAction::ShellCommand { label, command } => {
                            s.push_str(&format!(
                                "        shell {label:?} command={command:?}\n"
                            ));
                        }
                        VirtualAction::Unset => {}
                    }
                }
                s.push_str("    }\n\n");
            }

            // remap block
            let has_remaps = !out.key_remaps.is_empty() || !out.modifier_remaps.is_empty();
            if has_remaps {
                s.push_str("    remap {\n");
                for kr in &out.key_remaps {
                    let src = kc_display(kr.src_keycode);
                    let dst = kc_display(kr.dst_keycode);
                    s.push_str(&format!("        key {src} {dst}"));
                    if kr.src_modifier != 0 {
                        s.push_str(&format!(" src-mod={}", mod_display(kr.src_modifier)));
                    }
                    if kr.dst_modifier != 0 {
                        s.push_str(&format!(" dst-mod={}", mod_display(kr.dst_modifier)));
                    }
                    if kr.output_mask != 3 {
                        s.push_str(&format!(" outputs={}", kr.output_mask));
                    }
                    s.push('\n');
                }
                for mr in &out.modifier_remaps {
                    let src = mod_display(mr.src);
                    let dst = mod_display(mr.dst);
                    s.push_str(&format!("        modifier {src} {dst}\n"));
                }
                s.push_str("    }\n\n");
            }

            // layers block
            let cl = &out.caps_layer;
            if !cl.entries.is_empty() {
                s.push_str("    layers {\n");
                let passthrough = if cl.unmapped_passthrough { "" } else { " unmapped-passthrough=#false" };
                s.push_str(&format!("        caps{passthrough} {{\n"));
                for e in &cl.entries {
                    match e.entry_type {
                        CAPS_ENTRY_CHORD => {
                            let src = kc_display(e.src_keycode);
                            s.push_str(&format!("            chord {src}"));
                            // First key gets the modifier prefix; rest are plain.
                            let prefix = mod_prefix(e.dst_modifier);
                            let mut first = true;
                            for &k in e.dst_keycodes.iter().take_while(|&&k| k != 0) {
                                let key = kc_display(k);
                                if first {
                                    s.push_str(&format!(" {prefix}{key}"));
                                    first = false;
                                } else {
                                    s.push_str(&format!(" {key}"));
                                }
                            }
                            s.push('\n');
                        }
                        CAPS_ENTRY_VIRTUAL => {
                            let src = kc_display(e.src_keycode);
                            let label = out.virtual_actions
                                .get(e.vaction_idx as usize)
                                .and_then(|a| a.label())
                                .unwrap_or("?");
                            s.push_str(&format!("            action {src} {label:?}\n"));
                        }
                        CAPS_ENTRY_JUMP_A => {
                            s.push_str(&format!("            jump-a {}\n", kc_display(e.src_keycode)));
                        }
                        CAPS_ENTRY_JUMP_B => {
                            s.push_str(&format!("            jump-b {}\n", kc_display(e.src_keycode)));
                        }
                        CAPS_ENTRY_SWAP => {
                            s.push_str(&format!("            swap {}\n", kc_display(e.src_keycode)));
                        }
                        _ => {}
                    }
                }
                s.push_str("        }\n");
                s.push_str("    }\n");
            }

            // skip block
            if !out.skip.is_empty() {
                s.push_str("    skip {\n");
                for app in &out.skip {
                    s.push_str(&format!("        app {app:?}\n"));
                }
                s.push_str("    }\n");
            }

            s.push_str("}\n\n");
        }

        // git-sync block
        if let Some(remote) = &self.git_sync.remote {
            s.push_str(&format!("git-sync {{\n    remote {remote:?}\n}}\n\n"));
        }

        // sync block
        if !self.sync.apps.is_empty() || !self.sync.pairs.is_empty() {
            s.push_str("sync {\n");
            for app in &self.sync.apps {
                s.push_str(&format!("    app {app:?}\n"));
            }
            for pair in &self.sync.pairs {
                s.push_str(&format!("    pair {:?} {:?}\n", pair.local, pair.remote));
            }
            s.push_str("}\n");
        }

        s
    }

    // ── Hot-reload watcher ────────────────────────────────────────────────────

    /// Spawn a file watcher on `dualie.kdl`.  Returns a `watch::Receiver` that
    /// yields the latest parsed config whenever the file changes.
    pub fn watch() -> Result<tokio::sync::watch::Receiver<Self>> {
        let path = kdl_config_path();

        // Ensure the config directory exists.
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("creating config dir {}", dir.display()))?;
        }

        // Write a starter config if none exists yet.
        if !path.exists() {
            std::fs::write(&path, DEFAULT_CONFIG)
                .with_context(|| format!("writing default config to {}", path.display()))?;
            tracing::info!("created default config at {}", path.display());
        }

        let initial = Self::load_or_default()?;
        let (tx, rx) = tokio::sync::watch::channel(initial);

        tokio::task::spawn_blocking(move || {
            use notify::{RecommendedWatcher, RecursiveMode, Watcher};
            let (ftx, frx) = std::sync::mpsc::channel();
            let mut watcher: RecommendedWatcher = match notify::recommended_watcher(ftx) {
                Ok(w) => w,
                Err(e) => { tracing::error!("config watcher: {e}"); return; }
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
                        if !event.paths.iter().any(|p| p == &path) { continue; }
                        if matches!(event.kind,
                            notify::EventKind::Create(_) |
                            notify::EventKind::Modify(notify::event::ModifyKind::Data(_)) |
                            notify::EventKind::Modify(notify::event::ModifyKind::Any))
                        {
                            match Self::load_or_default() {
                                Ok(cfg) => {
                                    tracing::info!("config reloaded");
                                    let _ = tx.send(cfg);
                                    crate::git_sync::trigger_commit();
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
}

// ── Sync block parser ─────────────────────────────────────────────────────────

fn parse_sync(doc: &KdlDocument, sync: &mut SyncConfig) -> Result<()> {
    for node in doc.nodes() {
        match node.name().value() {
            "app" => {
                let name = kdl_arg_str(node, 0)
                    .ok_or_else(|| anyhow::anyhow!("sync app: requires a name argument"))?;
                sync.add_app(name);
            }
            "pair" => {
                let local = kdl_arg_str(node, 0)
                    .ok_or_else(|| anyhow::anyhow!("sync pair: requires local path"))?
                    .to_owned();
                let remote = kdl_arg_str(node, 1)
                    .ok_or_else(|| anyhow::anyhow!("sync pair: requires remote path"))?
                    .to_owned();
                sync.pairs.push(ManualSyncPair { local, remote });
            }
            other => tracing::warn!("sync: unknown node {other:?}"),
        }
    }
    Ok(())
}

// ── Machine block parser ──────────────────────────────────────────────────────

fn parse_machine(doc: &KdlDocument, mc: &mut MachineConfig) -> Result<()> {
    for node in doc.nodes() {
        match node.name().value() {
            "actions" => {
                if let Some(children) = node.children() {
                    parse_actions(children, mc)?;
                }
            }
            "remap" => {
                if let Some(children) = node.children() {
                    parse_remap(children, mc)?;
                }
            }
            "layers" => {
                if let Some(children) = node.children() {
                    parse_layers(children, mc)?;
                }
            }
            "skip" => {
                if let Some(children) = node.children() {
                    for child in children.nodes() {
                        if child.name().value() == "app" {
                            if let Some(s) = kdl_arg_str(child, 0) {
                                mc.skip.push(s.to_owned());
                            }
                        } else {
                            tracing::warn!("machine skip: unknown node {:?}", child.name().value());
                        }
                    }
                }
            }
            other => tracing::warn!("unknown machine node: {other}"),
        }
    }
    Ok(())
}

// ── Actions parser ────────────────────────────────────────────────────────────

fn parse_actions(doc: &KdlDocument, out: &mut MachineConfig) -> Result<()> {
    let mut slot = 0usize;
    for node in doc.nodes() {
        if slot >= DUALIE_VKEY_COUNT {
            bail!("too many actions (max {DUALIE_VKEY_COUNT})");
        }
        let label = kdl_arg_str(node, 0)
            .ok_or_else(|| anyhow::anyhow!("{} requires label as first arg", node.name().value()))?
            .to_owned();
        out.virtual_actions[slot] = match node.name().value() {
            "launch" => VirtualAction::AppLaunch {
                app_id: kdl_prop_str(node, "app-id")
                    .ok_or_else(|| anyhow::anyhow!("launch requires app-id="))?
                    .to_owned(),
                label,
            },
            "shell" => VirtualAction::ShellCommand {
                command: kdl_prop_str(node, "command")
                    .ok_or_else(|| anyhow::anyhow!("shell requires command="))?
                    .to_owned(),
                label,
            },
            other => bail!("unknown action type: {other}; expected launch or shell"),
        };
        slot += 1;
    }
    Ok(())
}

// ── Remap parser ──────────────────────────────────────────────────────────────

fn parse_remap(doc: &KdlDocument, out: &mut MachineConfig) -> Result<()> {
    for node in doc.nodes() {
        match node.name().value() {
            "key" => {
                let src = kdl_arg_as_keycode(node, 0)
                    .ok_or_else(|| anyhow::anyhow!("key: invalid src keycode"))?;
                let dst = kdl_arg_as_keycode(node, 1)
                    .ok_or_else(|| anyhow::anyhow!("key: invalid dst keycode"))?;
                out.key_remaps.push(KeyRemap {
                    src_keycode:  src,
                    dst_keycode:  dst,
                    src_modifier: kdl_prop_as_modifier(node, "src-mod").unwrap_or(0),
                    dst_modifier: kdl_prop_as_modifier(node, "dst-mod").unwrap_or(0),
                    output_mask:  kdl_prop_u8(node, "outputs").unwrap_or(3),
                    flags:        0,
                });
            }
            "modifier" => {
                let src = kdl_arg_as_modifier(node, 0)
                    .ok_or_else(|| anyhow::anyhow!("modifier: unknown src modifier name"))?;
                let dst = kdl_arg_as_modifier(node, 1)
                    .ok_or_else(|| anyhow::anyhow!("modifier: unknown dst modifier name"))?;
                out.modifier_remaps.push(ModifierRemap { src, dst });
            }
            other => tracing::warn!("unknown remap node: {other}"),
        }
    }
    Ok(())
}

// ── Layers parser ─────────────────────────────────────────────────────────────

fn parse_layers(doc: &KdlDocument, out: &mut MachineConfig) -> Result<()> {
    for node in doc.nodes() {
        match node.name().value() {
            "caps" => {
                out.caps_layer.unmapped_passthrough =
                    kdl_prop_bool(node, "unmapped-passthrough").unwrap_or(true);
                if let Some(children) = node.children() {
                    parse_caps(children, out)?;
                }
            }
            other => tracing::warn!("unknown layer: {other}"),
        }
    }
    Ok(())
}

fn parse_caps(doc: &KdlDocument, out: &mut MachineConfig) -> Result<()> {
    for node in doc.nodes() {
        let name = node.name().value();

        // chord/action/jump-a/jump-b/swap all require a src keycode as first arg
        let src = kdl_arg_as_keycode(node, 0)
            .ok_or_else(|| anyhow::anyhow!("{name}: invalid src keycode"))?;
        let output_mask = kdl_prop_u8(node, "outputs").unwrap_or(3);

        let entry = match name {
            "chord" => {
                // Each positional arg after src is a `[mods_]key` token or a raw keycode.
                // e.g. `ctrl_t`, `ctrl_shift_a`, `0x08`, `e`
                let mut dst = [0u8; 4];
                let mut n_dst = 0usize;
                let mut dst_modifier = 0u8;

                for arg_idx in 1.. {
                    match kdl_arg_as_mod_key(node, arg_idx) {
                        Some((m, kc)) => {
                            dst_modifier |= m;
                            if n_dst < 4 { dst[n_dst] = kc; n_dst += 1; }
                        }
                        None => break,
                    }
                }

                CapsLayerEntry {
                    src_keycode:  src,
                    entry_type:   CAPS_ENTRY_CHORD,
                    output_mask,
                    dst_modifier,
                    dst_keycodes: dst,
                    vaction_idx:  0,
                }
            }

            "action" => {
                // second arg is the action label; resolve to slot via virtual_actions
                let label = kdl_arg_str(node, 1)
                    .ok_or_else(|| anyhow::anyhow!("action: missing label arg"))?;
                let slot = out.virtual_actions.iter()
                    .position(|a| a.label() == Some(label))
                    .ok_or_else(|| anyhow::anyhow!("action: label {label:?} not found in actions block"))?;
                CapsLayerEntry {
                    src_keycode:  src,
                    entry_type:   CAPS_ENTRY_VIRTUAL,
                    output_mask,
                    vaction_idx:  slot as u8,
                    ..Default::default()
                }
            }

            "jump-a" => CapsLayerEntry {
                src_keycode: src, entry_type: CAPS_ENTRY_JUMP_A,
                output_mask, ..Default::default()
            },
            "jump-b" => CapsLayerEntry {
                src_keycode: src, entry_type: CAPS_ENTRY_JUMP_B,
                output_mask, ..Default::default()
            },
            "swap" => CapsLayerEntry {
                src_keycode: src, entry_type: CAPS_ENTRY_SWAP,
                output_mask, ..Default::default()
            },
            "clip-pull" => CapsLayerEntry {
                src_keycode: src, entry_type: CAPS_ENTRY_CLIP_PULL,
                output_mask, ..Default::default()
            },
            other => {
                tracing::warn!("unknown caps entry: {other}");
                continue;
            }
        };

        if out.caps_layer.entries.len() < CAPS_LAYER_MAX {
            out.caps_layer.entries.push(entry);
        } else {
            tracing::warn!("caps-layer limit ({CAPS_LAYER_MAX}) reached");
        }
    }
    Ok(())
}

// ── Key name tables ───────────────────────────────────────────────────────────

/// Resolve a key name or single character to a HID keycode.
/// Accepts: single letter (a-z), single digit (0-9), or named key.
pub fn keycode_by_name(name: &str) -> Option<u8> {
    // Single character shortcuts
    if name.len() == 1 {
        let c = name.chars().next().unwrap();
        if c.is_ascii_lowercase() {
            return Some(0x04 + (c as u8 - b'a'));
        }
        if let Some(kc) = match c {
            '0' => Some(0x27u8),
            '1'..='9' => Some(0x1E + (c as u8 - b'1')),
            _ => None,
        } { return Some(kc); }
    }

    // Named keys (case-insensitive via lowercase match)
    match name.to_ascii_lowercase().as_str() {
        "enter" | "return" => Some(0x28),
        "esc"   | "escape" => Some(0x29),
        "backspace"        => Some(0x2A),
        "tab"              => Some(0x2B),
        "space"            => Some(0x2C),
        "minus" | "-"      => Some(0x2D),
        "equals" | "="     => Some(0x2E),
        "lbracket" | "["   => Some(0x2F),
        "rbracket" | "]"   => Some(0x30),
        "backslash" | "\\" => Some(0x31),
        "semicolon" | ";"  => Some(0x33),
        "quote"  | "'"     => Some(0x34),
        "grave"  | "`"     => Some(0x35),
        "comma"  | ","     => Some(0x36),
        "period" | "."     => Some(0x37),
        "slash"  | "/"     => Some(0x38),
        "capslock"         => Some(0x39),
        "f1"  => Some(0x3A), "f2"  => Some(0x3B), "f3"  => Some(0x3C),
        "f4"  => Some(0x3D), "f5"  => Some(0x3E), "f6"  => Some(0x3F),
        "f7"  => Some(0x40), "f8"  => Some(0x41), "f9"  => Some(0x42),
        "f10" => Some(0x43), "f11" => Some(0x44), "f12" => Some(0x45),
        "printscreen"      => Some(0x46),
        "scrolllock"       => Some(0x47),
        "pause"            => Some(0x48),
        "insert"           => Some(0x49),
        "home"             => Some(0x4A),
        "pageup"           => Some(0x4B),
        "delete" | "del"   => Some(0x4C),
        "end"              => Some(0x4D),
        "pagedown"         => Some(0x4E),
        "right"            => Some(0x4F),
        "left"             => Some(0x50),
        "down"             => Some(0x51),
        "up"               => Some(0x52),
        "mute"             => Some(0x7F),
        "volup" | "volumeup"   => Some(0x80),
        "voldown" | "volumedown" => Some(0x81),
        _ => None,
    }
}

/// Reverse: HID keycode → display name (used by `to_kdl_string`).
#[allow(dead_code)]
fn kc_display(kc: u8) -> String {
    // a-z
    if (0x04..=0x1D).contains(&kc) {
        return ((b'a' + kc - 0x04) as char).to_string();
    }
    // 1-9
    if (0x1E..=0x26).contains(&kc) {
        return ((b'1' + kc - 0x1E) as char).to_string();
    }
    match kc {
        0x27 => "0".into(),
        0x28 => "enter".into(),    0x29 => "esc".into(),
        0x2A => "backspace".into(),0x2B => "tab".into(),
        0x2C => "space".into(),    0x39 => "capslock".into(),
        0x3A => "f1".into(),  0x3B => "f2".into(),  0x3C => "f3".into(),
        0x3D => "f4".into(),  0x3E => "f5".into(),  0x3F => "f6".into(),
        0x40 => "f7".into(),  0x41 => "f8".into(),  0x42 => "f9".into(),
        0x43 => "f10".into(), 0x44 => "f11".into(), 0x45 => "f12".into(),
        0x49 => "insert".into(),   0x4A => "home".into(),
        0x4B => "pageup".into(),   0x4C => "delete".into(),
        0x4D => "end".into(),      0x4E => "pagedown".into(),
        0x4F => "right".into(),    0x50 => "left".into(),
        0x51 => "down".into(),     0x52 => "up".into(),
        0x7F => "mute".into(),
        0x80 => "volup".into(),    0x81 => "voldown".into(),
        n    => format!("0x{n:02X}"),
    }
}

fn modifier_by_name(name: &str) -> Option<u8> {
    match name {
        // Long forms
        "lctrl"  => Some(0x01), "lshift" => Some(0x02),
        "lalt"   => Some(0x04), "lmeta"  => Some(0x08),
        "rctrl"  => Some(0x10), "rshift" => Some(0x20),
        "ralt"   => Some(0x40), "rmeta"  => Some(0x80),
        // Short aliases (left-side by default)
        "ctrl"   => Some(0x01), "shift"  => Some(0x02),
        "alt"    => Some(0x04),
        "meta" | "cmd" | "win" | "super" => Some(0x08),
        _ => None,
    }
}

/// Single-modifier display name used in chord `mod_key` tokens.
#[allow(dead_code)]
fn mod_bit_name(bit: u8) -> &'static str {
    match bit {
        0x01 => "ctrl",   0x02 => "shift",
        0x04 => "alt",    0x08 => "meta",
        0x10 => "rctrl",  0x20 => "rshift",
        0x40 => "ralt",   0x80 => "rmeta",
        _    => "?",
    }
}

/// Build the `mod1_mod2_` prefix string for a modifier bitmask (e.g. `ctrl_shift_`).
#[allow(dead_code)]
fn mod_prefix(m: u8) -> String {
    if m == 0 { return String::new(); }
    let mut parts = Vec::new();
    for bit in [0x01u8, 0x02, 0x04, 0x08, 0x10, 0x20, 0x40, 0x80] {
        if m & bit != 0 { parts.push(mod_bit_name(bit)); }
    }
    parts.join("_") + "_"
}

/// Display a modifier bitmask for `modifier` remap entries (e.g. `lalt`).
#[allow(dead_code)]
fn mod_display(m: u8) -> String {
    match m {
        0x01 => "lctrl".into(),  0x02 => "lshift".into(),
        0x04 => "lalt".into(),   0x08 => "lmeta".into(),
        0x10 => "rctrl".into(),  0x20 => "rshift".into(),
        0x40 => "ralt".into(),   0x80 => "rmeta".into(),
        n    => format!("0x{n:02X}"),
    }
}

// ── Low-level KDL node helpers ────────────────────────────────────────────────

fn kdl_prop<'a>(node: &'a KdlNode, key: &str) -> Option<&'a KdlValue> {
    node.entries().iter()
        .find(|e| e.name().is_some_and(|n| n.value() == key))
        .map(|e| e.value())
}

fn kdl_arg<'a>(node: &'a KdlNode, idx: usize) -> Option<&'a KdlValue> {
    node.entries().iter()
        .filter(|e| e.name().is_none())
        .nth(idx)
        .map(|e| e.value())
}

/// Extract a string-or-identifier value (KDL v2: both quoted strings and bare identifiers).
fn kdl_val_as_str(v: &KdlValue) -> Option<&str> {
    v.as_string()
}

fn kdl_prop_str<'a>(node: &'a KdlNode, key: &str) -> Option<&'a str> {
    kdl_prop(node, key).and_then(|v| kdl_val_as_str(v))
}

fn kdl_prop_bool(node: &KdlNode, key: &str) -> Option<bool> {
    kdl_prop(node, key).and_then(|v| v.as_bool())
}

fn kdl_prop_u8(node: &KdlNode, key: &str) -> Option<u8> {
    kdl_prop(node, key)
        .and_then(|v| kdl_as_i64(v))
        .and_then(|n| u8::try_from(n).ok())
}

/// Get positional arg as string or identifier.
fn kdl_arg_str<'a>(node: &'a KdlNode, idx: usize) -> Option<&'a str> {
    kdl_arg(node, idx).and_then(|v| kdl_val_as_str(v))
}

/// Resolve a positional arg (integer keycode or named key) to a HID keycode.
fn kdl_arg_as_keycode(node: &KdlNode, idx: usize) -> Option<u8> {
    let v = kdl_arg(node, idx)?;
    if let Some(n) = kdl_as_i64(v) { return u8::try_from(n).ok(); }
    kdl_val_as_str(v).and_then(keycode_by_name)
}

/// Parse an underscore-combined `[mod_]key` token into `(modifier_bits, keycode)`.
/// Examples: `ctrl_t` → (0x01, 0x17), `ctrl_shift_a` → (0x03, 0x04), `e` → (0, 0x08).
/// Raw integers (0x04, 65) are also accepted and return modifier=0.
fn kdl_arg_as_mod_key(node: &KdlNode, idx: usize) -> Option<(u8, u8)> {
    let v = kdl_arg(node, idx)?;
    // Integer → plain keycode, no modifier
    if let Some(n) = kdl_as_i64(v) {
        return u8::try_from(n).ok().map(|kc| (0u8, kc));
    }
    // String/identifier → parse underscore-separated mod+key
    let s = kdl_val_as_str(v)?;
    parse_mod_key(s)
}

/// Split `ctrl_shift_a` into `(modifier_bits, keycode)`.
fn parse_mod_key(s: &str) -> Option<(u8, u8)> {
    let parts: Vec<&str> = s.split('_').collect();
    let mut modifier = 0u8;
    let mut key_start = 0usize;

    for (i, &part) in parts.iter().enumerate() {
        if let Some(m) = modifier_by_name(part) {
            modifier |= m;
            key_start = i + 1;
        } else {
            key_start = i;
            break;
        }
    }

    let key_name = parts[key_start..].join("_");
    let kc = keycode_by_name(&key_name)?;
    Some((modifier, kc))
}

/// Resolve a prop value (integer or name) to a modifier bitmask.
fn kdl_prop_as_modifier(node: &KdlNode, key: &str) -> Option<u8> {
    let v = kdl_prop(node, key)?;
    if let Some(n) = kdl_as_i64(v) { return u8::try_from(n).ok(); }
    kdl_val_as_str(v).and_then(modifier_by_name)
}

/// Resolve a positional arg as a modifier bitmask (name or integer).
fn kdl_arg_as_modifier(node: &KdlNode, idx: usize) -> Option<u8> {
    let v = kdl_arg(node, idx)?;
    if let Some(n) = kdl_as_i64(v) { return u8::try_from(n).ok(); }
    kdl_val_as_str(v).and_then(modifier_by_name)
}

fn kdl_as_i64(v: &KdlValue) -> Option<i64> {
    match v {
        KdlValue::Integer(n) => Some(*n as i64),
        _ => None,
    }
}

// ── Paths ─────────────────────────────────────────────────────────────────────

pub fn kdl_config_path() -> PathBuf {
    project_dirs().config_dir().join("dualie.kdl")
}

/// Path of `local.kdl` — machine-local overrides, never committed to git.
pub fn local_config_path() -> PathBuf {
    project_dirs().config_dir().join("local.kdl")
}

/// Legacy JSON config path — used only for `load_or_default` migration fallback.
fn json_config_path() -> PathBuf {
    project_dirs().config_dir().join("config.json")
}

fn project_dirs() -> ProjectDirs {
    ProjectDirs::from("dev", "dualie", "dualie")
        .expect("could not determine config directory")
}

// ── Local config (machine-specific, not git-tracked) ─────────────────────────

/// Machine-local settings parsed from `local.kdl`.
///
/// ```kdl
/// local {
///     machine-name "mbp-work"
///
///     git-sync {
///         repo-path "~/src/dotfiles"   // optional; overrides platform default
///     }
/// }
/// ```
#[derive(Debug, Clone)]
pub struct LocalConfig {
    /// Human-readable machine name embedded in git commit messages.
    pub machine_name: String,
    /// Override for the git repo directory.  `None` → use `git_sync::default_repo_dir()`.
    pub repo_path: Option<PathBuf>,
}

impl Default for LocalConfig {
    fn default() -> Self {
        Self { machine_name: hostname_fallback(), repo_path: None }
    }
}

impl LocalConfig {
    /// Load `local.kdl`, falling back to a hostname-derived default if absent.
    pub fn load() -> Self {
        let path = local_config_path();
        if !path.exists() {
            return Self::default();
        }
        match std::fs::read_to_string(&path) {
            Ok(src) => Self::from_kdl(&src).unwrap_or_else(|e| {
                tracing::warn!("local.kdl parse error: {e}");
                Self::default()
            }),
            Err(e) => {
                tracing::warn!("reading local.kdl: {e}");
                Self::default()
            }
        }
    }

    pub(crate) fn from_kdl(src: &str) -> Result<Self> {
        let doc = src.parse::<KdlDocument>()
            .map_err(|e| anyhow::anyhow!("{:?}", miette::Report::new(e)))?;

        let mut cfg = Self::default();

        for node in doc.nodes() {
            if node.name().value() != "local" {
                continue;
            }
            let Some(children) = node.children() else { continue };
            for child in children.nodes() {
                match child.name().value() {
                    "machine-name" => {
                        if let Some(s) = kdl_arg_str(child, 0) {
                            cfg.machine_name = s.to_owned();
                        }
                    }
                    "git-sync" => {
                        if let Some(gc) = child.children() {
                            for g in gc.nodes() {
                                if g.name().value() == "repo-path" {
                                    if let Some(s) = kdl_arg_str(g, 0) {
                                        cfg.repo_path = Some(expand_tilde(s));
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        Ok(cfg)
    }
}

fn hostname_fallback() -> String {
    let raw = gethostname::gethostname();
    let lossy = raw.to_string_lossy().into_owned();
    // Strip FQDN suffix — keep only the short hostname.
    lossy.split('.').next().unwrap_or(&lossy).to_owned()
}

fn expand_tilde(s: &str) -> PathBuf {
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(dirs) = directories::UserDirs::new() {
            return dirs.home_dir().join(rest);
        }
    }
    PathBuf::from(s)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE_KDL: &str = r#"
ports {
    a desk
    b laptop
}

machine desk {
    actions {
        launch "Slack" app-id="com.tinyspeck.slackmacgap"
        shell  "Terminal" command="open -a Terminal"
    }

    remap {
        key esc backspace
        key 0x39 0x29
        modifier lalt rctrl
    }

    layers {
        caps {
            chord  a e
            chord  b ctrl_t
            chord  c ctrl_shift_a
            action s "Slack"
            jump-a h
            jump-b k
            swap   n
        }
    }
}

machine laptop {}
"#;

    #[test]
    fn parse_actions() {
        let cfg = DualieConfig::from_kdl(EXAMPLE_KDL).expect("parse");
        let mc = cfg.resolve_port(0).expect("port 0 should resolve");
        assert_eq!(mc.virtual_actions[0], VirtualAction::AppLaunch {
            app_id: "com.tinyspeck.slackmacgap".into(),
            label:  "Slack".into(),
        });
        assert_eq!(mc.virtual_actions[1], VirtualAction::ShellCommand {
            command: "open -a Terminal".into(),
            label:   "Terminal".into(),
        });
    }

    #[test]
    fn parse_remap() {
        let cfg = DualieConfig::from_kdl(EXAMPLE_KDL).expect("parse");
        let mc = cfg.resolve_port(0).expect("port 0 should resolve");
        let kr = &mc.key_remaps;
        assert_eq!(kr.len(), 2);
        // "esc" → 0x29, "backspace" → 0x2A
        assert_eq!(kr[0].src_keycode, 0x29);
        assert_eq!(kr[0].dst_keycode, 0x2A);
        // 0x39 → 0x29
        assert_eq!(kr[1].src_keycode, 0x39);
        assert_eq!(kr[1].dst_keycode, 0x29);
        // modifier lalt → rctrl
        let mr = &mc.modifier_remaps;
        assert_eq!(mr[0].src, 0x04); // lalt
        assert_eq!(mr[0].dst, 0x10); // rctrl
    }

    #[test]
    fn parse_caps_layer() {
        let cfg = DualieConfig::from_kdl(EXAMPLE_KDL).expect("parse");
        let mc = cfg.resolve_port(0).expect("port 0 should resolve");
        let cl = &mc.caps_layer;
        assert_eq!(cl.entries.len(), 7);

        // chord a e  → src=a, dst=[e], mod=0
        assert_eq!(cl.entries[0].entry_type, CAPS_ENTRY_CHORD);
        assert_eq!(cl.entries[0].src_keycode, 0x04); // a
        assert_eq!(cl.entries[0].dst_keycodes[0], 0x08); // e
        assert_eq!(cl.entries[0].dst_modifier, 0);

        // chord b ctrl_t → src=b, dst=[t], mod=ctrl(0x01)
        assert_eq!(cl.entries[1].src_keycode, 0x05); // b
        assert_eq!(cl.entries[1].dst_modifier, 0x01); // ctrl
        assert_eq!(cl.entries[1].dst_keycodes[0], 0x17); // t

        // chord c ctrl_shift_a → src=c, dst=[a], mod=ctrl|shift(0x03)
        assert_eq!(cl.entries[2].dst_modifier, 0x03);
        assert_eq!(cl.entries[2].dst_keycodes[0], 0x04); // a

        // action s "Slack" → slot 0
        assert_eq!(cl.entries[3].entry_type, CAPS_ENTRY_VIRTUAL);
        assert_eq!(cl.entries[3].src_keycode, 0x16); // s
        assert_eq!(cl.entries[3].vaction_idx, 0);

        // jump-a h
        assert_eq!(cl.entries[4].entry_type, CAPS_ENTRY_JUMP_A);
        assert_eq!(cl.entries[4].src_keycode, 0x0B); // h
    }

    #[test]
    fn parse_mod_key_fn() {
        assert_eq!(parse_mod_key("e"),            Some((0x00, 0x08)));
        assert_eq!(parse_mod_key("ctrl_t"),       Some((0x01, 0x17)));
        assert_eq!(parse_mod_key("ctrl_shift_a"), Some((0x03, 0x04)));
        assert_eq!(parse_mod_key("alt_f4"),       Some((0x04, 0x3D)));
        assert_eq!(parse_mod_key("meta_space"),   Some((0x08, 0x2C)));
        assert_eq!(parse_mod_key("0x04"),         None); // hex handled separately
    }

    #[test]
    fn machine_and_ports_parse() {
        let src = "ports {\n    a desk\n    b laptop\n}\nmachine desk {}\nmachine laptop {}";
        let cfg = DualieConfig::from_kdl(src).expect("parse");
        assert_eq!(cfg.ports[0].as_deref(), Some("desk"));
        assert_eq!(cfg.ports[1].as_deref(), Some("laptop"));
        assert!(cfg.machines.contains_key("desk"));
        assert!(cfg.machines.contains_key("laptop"));
    }

    #[test]
    fn ports_referencing_unknown_machine_is_error() {
        let src = "ports { a ghost }\nmachine desk {}";
        assert!(DualieConfig::from_kdl(src).is_err(), "unknown machine should error");
    }

    #[test]
    fn roundtrip_empty() {
        let original = DualieConfig::default();
        let kdl = original.to_kdl_string();
        let restored = DualieConfig::from_kdl(&kdl).expect("parse roundtrip");
        // Default config has no ports or machines.
        assert!(restored.ports[0].is_none());
        assert!(restored.machines.is_empty());
    }

    #[test]
    fn keycode_names() {
        assert_eq!(keycode_by_name("a"), Some(0x04));
        assert_eq!(keycode_by_name("z"), Some(0x1D));
        assert_eq!(keycode_by_name("1"), Some(0x1E));
        assert_eq!(keycode_by_name("0"), Some(0x27));
        assert_eq!(keycode_by_name("esc"), Some(0x29));
        assert_eq!(keycode_by_name("left"), Some(0x50));
        assert_eq!(keycode_by_name("volup"), Some(0x80));
        assert_eq!(keycode_by_name("f12"), Some(0x45));
    }

    #[test]
    fn cbor_roundtrip() {
        let original = DualieConfig::default();
        let bytes    = original.to_cbor().expect("to_cbor");
        let restored = DualieConfig::from_cbor(&bytes).expect("from_cbor");
        let j1 = serde_json::to_string(&original).unwrap();
        let j2 = serde_json::to_string(&restored).unwrap();
        assert_eq!(j1, j2);
    }

    #[test]
    fn to_kdl_string_roundtrip() {
        // Parse the full example, serialise to KDL, re-parse — structures must match.
        let original = DualieConfig::from_kdl(EXAMPLE_KDL).expect("parse");
        let kdl = original.to_kdl_string();
        let restored = DualieConfig::from_kdl(&kdl)
            .unwrap_or_else(|e| panic!("re-parse after to_kdl_string failed:\n{e}\n\nKDL:\n{kdl}"));

        let orig_mc = original.resolve_port(0).expect("port 0 should resolve");
        let rest_mc = restored.resolve_port(0).expect("port 0 should resolve after roundtrip");

        // Key remaps preserved
        assert_eq!(orig_mc.key_remaps.len(), rest_mc.key_remaps.len(), "key remap count mismatch");
        for (a, b) in orig_mc.key_remaps.iter().zip(rest_mc.key_remaps.iter()) {
            assert_eq!(a.src_keycode, b.src_keycode, "src_keycode mismatch");
            assert_eq!(a.dst_keycode, b.dst_keycode, "dst_keycode mismatch");
        }

        // Modifier remaps preserved
        assert_eq!(orig_mc.modifier_remaps.len(), rest_mc.modifier_remaps.len());

        // Caps layer entries preserved
        assert_eq!(
            orig_mc.caps_layer.entries.len(),
            rest_mc.caps_layer.entries.len(),
            "caps layer entry count mismatch"
        );
        for (a, b) in orig_mc.caps_layer.entries.iter().zip(rest_mc.caps_layer.entries.iter()) {
            assert_eq!(a.entry_type,  b.entry_type,  "entry_type mismatch");
            assert_eq!(a.src_keycode, b.src_keycode, "src_keycode mismatch");
        }

        // Action labels preserved
        let orig_labels: Vec<_> = orig_mc.virtual_actions.iter().filter_map(|a| a.label()).collect();
        let rest_labels: Vec<_> = rest_mc.virtual_actions.iter().filter_map(|a| a.label()).collect();
        assert_eq!(orig_labels, rest_labels, "action labels mismatch");
    }

    // ── git-sync block ────────────────────────────────────────────────────────

    #[test]
    fn parse_git_sync_remote() {
        let src = r#"
git-sync {
    remote "git@github.com:user/configs.git"
}
"#;
        let cfg = DualieConfig::from_kdl(src).expect("parse");
        assert_eq!(
            cfg.git_sync.remote.as_deref(),
            Some("git@github.com:user/configs.git"),
        );
    }

    #[test]
    fn git_sync_absent_defaults_to_none() {
        let cfg = DualieConfig::from_kdl("").expect("parse");
        assert!(cfg.git_sync.remote.is_none());
    }

    #[test]
    fn git_sync_roundtrip() {
        let mut cfg = DualieConfig::default();
        cfg.git_sync.remote = Some("git@github.com:user/configs.git".into());
        let kdl = cfg.to_kdl_string();
        let restored = DualieConfig::from_kdl(&kdl).expect("re-parse");
        assert_eq!(cfg.git_sync, restored.git_sync);
    }

    // ── LocalConfig ───────────────────────────────────────────────────────────

    #[test]
    fn local_config_parse_machine_name() {
        let src = r#"local { machine-name "mbp-work" }"#;
        let lc = LocalConfig::from_kdl(src).expect("parse");
        assert_eq!(lc.machine_name, "mbp-work");
        assert!(lc.repo_path.is_none());
    }

    #[test]
    fn local_config_parse_repo_path() {
        let src = r#"
local {
    machine-name "dev"
    git-sync {
        repo-path "/tmp/dotfiles"
    }
}
"#;
        let lc = LocalConfig::from_kdl(src).expect("parse");
        assert_eq!(lc.machine_name, "dev");
        assert_eq!(lc.repo_path.as_deref(), Some(std::path::Path::new("/tmp/dotfiles")));
    }

    #[test]
    fn local_config_empty_falls_back_to_hostname() {
        let lc = LocalConfig::from_kdl("").expect("parse");
        // hostname_fallback never returns an empty string
        assert!(!lc.machine_name.is_empty());
    }

    #[test]
    fn unknown_port_label_errors() {
        // port "c" is not valid — only a and b
        let src = "ports {\n    c desk\n}\nmachine desk {}";
        assert!(DualieConfig::from_kdl(src).is_err(), "unknown port should error");
    }

    #[test]
    fn invalid_kdl_syntax_errors() {
        // Missing closing brace
        let src = "machine desk {\n    remap {\n        key esc backspace\n";
        assert!(DualieConfig::from_kdl(src).is_err());
    }

}
