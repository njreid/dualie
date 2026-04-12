use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// ── Constants ─────────────────────────────────────────────────────────────────

/// Number of virtual action slots (0-31).
/// Previously these mapped to F13–F24 + other HID keycodes; now the RP2040
/// sends `DualieMessage::VirtualAction { slot }` over CDC-ACM serial directly.
pub const DUALIE_VKEY_COUNT: usize = 32;

// ── Virtual action definitions ────────────────────────────────────────────────

/// The type of action the daemon should perform when a virtual key fires.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum VirtualAction {
    /// Launch or focus an application by its platform ID.
    AppLaunch {
        /// Platform-specific identifier:
        ///   macOS  – bundle ID, e.g. "com.tinyspeck.slackmacgap"
        ///   Linux  – .desktop basename, e.g. "slack"
        app_id: String,
        /// Human-readable label for the UI
        label: String,
    },
    /// Run a shell command
    ShellCommand {
        command: String,
        label: String,
    },
    /// Placeholder / unassigned slot
    Unset,
}

impl Default for VirtualAction {
    fn default() -> Self { Self::Unset }
}

// ── Modifier remaps ───────────────────────────────────────────────────────────

/// HID modifier byte bit positions (matches firmware KEYBOARD_MODIFIER_* values).
///
/// Bit 0 = LCtrl, 1 = LShift, 2 = LAlt, 3 = LMeta,
/// Bit 4 = RCtrl, 5 = RShift, 6 = RAlt, 7 = RMeta
#[allow(dead_code)]
pub const MOD_LCTRL:  u8 = 0x01;
#[allow(dead_code)]
pub const MOD_LSHIFT: u8 = 0x02;
#[allow(dead_code)]
pub const MOD_LALT:   u8 = 0x04;
#[allow(dead_code)]
pub const MOD_LMETA:  u8 = 0x08;
#[allow(dead_code)]
pub const MOD_RCTRL:  u8 = 0x10;
#[allow(dead_code)]
pub const MOD_RSHIFT: u8 = 0x20;
#[allow(dead_code)]
pub const MOD_RALT:   u8 = 0x40;
#[allow(dead_code)]
pub const MOD_RMETA:  u8 = 0x80;

/// Remap a set of modifier bits to a different set.
/// Applied to every HID report for the active output, without needing a
/// specific key to be pressed (serialised into the key_remaps table with
/// flags = REMAP_FLAG_MOD_ONLY).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModifierRemap {
    /// Modifier bitmask that must all be present to trigger (e.g. 0x01 = LCtrl)
    pub src: u8,
    /// Modifier bitmask to emit in their place
    pub dst: u8,
}

// ── Key remaps ────────────────────────────────────────────────────────────────

/// One row of the per-output key-remap table (mirrors firmware key_remap_t).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct KeyRemap {
    /// HID keycode to match (0 = unused row)
    pub src_keycode:  u8,
    /// HID keycode to emit instead
    pub dst_keycode:  u8,
    /// Modifier byte that must also be present (0 = any)
    #[serde(default)]
    pub src_modifier: u8,
    /// Modifier byte to emit (replaces src modifiers when non-zero)
    #[serde(default)]
    pub dst_modifier: u8,
    /// Bitmask: bit 0 = output A, bit 1 = output B (3 = both)
    #[serde(default = "default_output_mask")]
    pub output_mask:  u8,
    /// Reserved flags (must match firmware key_remap_t.flags)
    #[serde(default)]
    pub flags:        u8,
}

fn default_output_mask() -> u8 { 3 }

// ── Caps layer ────────────────────────────────────────────────────────────────

pub const CAPS_LAYER_MAX: usize = 32;

/// entry_type values (match firmware CAPS_ENTRY_* constants)
pub const CAPS_ENTRY_CHORD:   u8 = 0;
pub const CAPS_ENTRY_VIRTUAL: u8 = 1;
#[allow(dead_code)]
pub const CAPS_ENTRY_JUMP_A:  u8 = 2;  // switch to output A, consume keypress
#[allow(dead_code)]
pub const CAPS_ENTRY_JUMP_B:  u8 = 3;  // switch to output B, consume keypress
#[allow(dead_code)]
pub const CAPS_ENTRY_SWAP:    u8 = 4;  // toggle output, consume keypress

/// One entry in the caps-layer table (mirrors firmware caps_layer_entry_t).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapsLayerEntry {
    /// Source key that triggers this entry (0 = unused)
    pub src_keycode:  u8,
    /// CAPS_ENTRY_CHORD (0) or CAPS_ENTRY_VIRTUAL (1)
    #[serde(default)]
    pub entry_type:   u8,
    /// Bitmask of outputs this entry applies to
    #[serde(default = "default_output_mask")]
    pub output_mask:  u8,
    /// Modifier byte to hold while sending dst_keycodes
    #[serde(default)]
    pub dst_modifier: u8,
    /// Chord: up to 4 simultaneous dest keycodes (ignored when entry_type=1)
    #[serde(default)]
    pub dst_keycodes: [u8; 4],
    /// Virtual: vaction slot index (ignored when entry_type=0)
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
    /// If true, keys with no caps-layer entry are passed through unchanged;
    /// if false, they are swallowed while CapsLock is held.
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

// ── Per-output config ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputDaemonConfig {
    /// Virtual action definitions indexed by vaction_idx (0-31)
    #[serde(default = "default_actions")]
    pub virtual_actions: Vec<VirtualAction>,
    /// Key remap table
    #[serde(default)]
    pub key_remaps: Vec<KeyRemap>,
    /// Modifier-only remaps (e.g. LCtrl → LAlt), applied to every report
    #[serde(default)]
    pub modifier_remaps: Vec<ModifierRemap>,
    /// Caps-layer mapping table
    #[serde(default)]
    pub caps_layer: CapsLayer,
}

fn default_actions() -> Vec<VirtualAction> {
    vec![VirtualAction::Unset; DUALIE_VKEY_COUNT]
}

impl Default for OutputDaemonConfig {
    fn default() -> Self {
        Self {
            virtual_actions: default_actions(),
            key_remaps:      Vec::new(),
            modifier_remaps: Vec::new(),
            caps_layer:      CapsLayer::default(),
        }
    }
}

// ── Top-level daemon config ───────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DualieConfig {
    pub version: u32,
    /// Daemon-side config per output (index 0 = OUTPUT_A, 1 = OUTPUT_B)
    #[serde(default)]
    pub outputs: [OutputDaemonConfig; 2],
}

impl Default for DualieConfig {
    fn default() -> Self {
        Self {
            version: 1,
            outputs: [
                OutputDaemonConfig::default(),
                OutputDaemonConfig::default(),
            ],
        }
    }
}

impl DualieConfig {
    pub fn load_or_default() -> Result<Self> {
        let path = config_path();
        if path.exists() {
            let raw = std::fs::read_to_string(&path)
                .with_context(|| format!("reading {}", path.display()))?;
            let cfg: Self = serde_json::from_str(&raw)
                .with_context(|| format!("parsing {}", path.display()))?;
            Ok(cfg)
        } else {
            Ok(Self::default())
        }
    }

    pub fn save(&self) -> Result<()> {
        let path = config_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, json)
            .with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    }

    /// Export the full config as a CBOR blob for download.
    pub fn to_cbor(&self) -> Result<Vec<u8>> {
        let mut buf = Vec::new();
        ciborium::into_writer(self, &mut buf)?;
        Ok(buf)
    }

    /// Import config from a CBOR blob (uploaded from browser).
    pub fn from_cbor(bytes: &[u8]) -> Result<Self> {
        let cfg: Self = ciborium::from_reader(bytes)?;
        Ok(cfg)
    }
}

// ── Paths ─────────────────────────────────────────────────────────────────────

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_roundtrip() {
        let original = DualieConfig::default();
        let json     = serde_json::to_string(&original).expect("serialize");
        let restored: DualieConfig = serde_json::from_str(&json).expect("deserialize");
        // Compare via re-serialization (DualieConfig doesn't derive PartialEq)
        let json2 = serde_json::to_string(&restored).expect("serialize restored");
        assert_eq!(json, json2, "JSON roundtrip produced different output");
    }

    #[test]
    fn cbor_roundtrip() {
        let original = DualieConfig::default();
        let bytes    = original.to_cbor().expect("to_cbor");
        let restored = DualieConfig::from_cbor(&bytes).expect("from_cbor");
        let json_orig    = serde_json::to_string(&original).expect("json orig");
        let json_restored = serde_json::to_string(&restored).expect("json restored");
        assert_eq!(json_orig, json_restored, "CBOR roundtrip produced different output");
    }

    #[test]
    fn virtual_action_serialization_tag() {
        let action = VirtualAction::AppLaunch {
            app_id: "com.foo".into(),
            label:  "Foo".into(),
        };
        let json = serde_json::to_string(&action).expect("serialize");
        let v: serde_json::Value = serde_json::from_str(&json).expect("parse");
        assert_eq!(v["type"], "app_launch", "type tag should be app_launch");
        assert_eq!(v["app_id"], "com.foo");
        assert_eq!(v["label"], "Foo");
    }

}

pub fn config_path() -> PathBuf {
    project_dirs().config_dir().join("config.json")
}

fn project_dirs() -> ProjectDirs {
    ProjectDirs::from("dev", "dualie", "dualie")
        .expect("could not determine config directory")
}
