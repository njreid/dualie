/// intercept/remap.rs — Platform-independent key remap logic.
///
/// Mirrors the firmware's `process_caps_layer()` but runs on all locally
/// attached keyboards.  Pure logic — no I/O, no platform deps, fully tested.
///
/// # Key/modifier representation
///
/// All keys are USB HID keycodes (Usage Page 0x07).
/// Modifiers are tracked as a bitmask (same layout as the HID report modifier byte):
///   0x01 lctrl  0x02 lshift  0x04 lalt  0x08 lmeta
///   0x10 rctrl  0x20 rshift  0x40 ralt  0x80 rmeta
///
/// The caller converts between evdev ↔ HID around every call to `process_key`.

use std::collections::{HashMap, HashSet};

use tracing::debug;

use crate::config::{
    MachineConfig,
    CAPS_ENTRY_CHORD, CAPS_ENTRY_CLIP_PULL, CAPS_ENTRY_JUMP_A, CAPS_ENTRY_JUMP_B,
    CAPS_ENTRY_SWAP, CAPS_ENTRY_VIRTUAL,
};

// ── Types ─────────────────────────────────────────────────────────────────────

/// evdev value field: key released.
pub const VALUE_UP: i32 = 0;
/// evdev value field: key pressed.
pub const VALUE_DOWN: i32 = 1;
/// evdev value field: key auto-repeat.
pub const VALUE_REPEAT: i32 = 2;

/// A single synthetic key event to inject into uinput.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntheticKey {
    /// USB HID keycode (0x04–0xFF).  0 means modifier-only report.
    pub hid: u8,
    /// Modifier bitmask active at the time of this event.
    pub modifiers: u8,
    /// VALUE_DOWN, VALUE_UP, or VALUE_REPEAT.
    pub value: i32,
}

/// Result of processing one evdev key event.
#[derive(Debug, Default)]
pub struct ProcessResult {
    /// Synthetic key events to inject (in order).  Empty means event consumed.
    pub events: Vec<SyntheticKey>,
    /// If Some, switch the active output to this index (0 = A, 1 = B, …).
    pub switch_output: Option<u8>,
    /// If Some, fire this virtual action slot number.
    pub fire_action: Option<u8>,
    /// If true, request the other machine's clipboard via serial.
    pub clip_pull: bool,
}

// ── Compiled config ───────────────────────────────────────────────────────────

/// Pre-compiled form of `OutputDaemonConfig` for fast per-event lookup.
#[derive(Debug, Default, Clone)]
pub struct CompiledOutputConfig {
    /// key_remaps[src_hid] = (dst_hid, required_src_modifier_mask)
    key_remaps: HashMap<u8, (u8, u8)>,
    /// modifier_remaps[src_bit] = dst_bit
    modifier_remaps: HashMap<u8, u8>,
    /// caps_chords[src_hid] = (dst_hid, dst_modifier)
    caps_chords: HashMap<u8, (u8, u8)>,
    /// caps_actions[src_hid] = vaction_idx
    caps_actions: HashMap<u8, u8>,
    /// caps_jump[src_hid] = target output index
    caps_jump: HashMap<u8, u8>,
    /// caps_swap: set of src_hid keys that toggle the active output
    caps_swap: HashSet<u8>,
    /// caps_clip_pull: set of src_hid keys that trigger a clipboard pull
    caps_clip_pull: HashSet<u8>,
    /// action labels for logging
    pub action_labels: Vec<String>,
    /// number of outputs (for swap wrapping)
    pub output_count: u8,
    /// index of this output (0-based)
    pub output_index: u8,
    /// whether unmapped caps+key events pass through (default true)
    pub unmapped_passthrough: bool,
}

impl CompiledOutputConfig {
    /// Compile an `OutputDaemonConfig` for the given output index.
    ///
    /// `output_index`: 0 for A, 1 for B.
    /// `output_count`: total number of outputs (for swap wrapping).
    pub fn from_config(cfg: &MachineConfig, output_index: u8, output_count: u8) -> Self {
        let mut out = CompiledOutputConfig {
            output_index,
            output_count,
            unmapped_passthrough: cfg.caps_layer.unmapped_passthrough,
            ..Default::default()
        };

        let bit = 1u8 << output_index;

        for kr in &cfg.key_remaps {
            if kr.output_mask & bit != 0 {
                out.key_remaps.insert(kr.src_keycode, (kr.dst_keycode, kr.src_modifier));
            }
        }

        for mr in &cfg.modifier_remaps {
            out.modifier_remaps.insert(mr.src, mr.dst);
        }

        for entry in &cfg.caps_layer.entries {
            if entry.output_mask & bit == 0 {
                continue;
            }
            match entry.entry_type {
                CAPS_ENTRY_CHORD => {
                    let dst_hid = entry.dst_keycodes[0];
                    out.caps_chords.insert(entry.src_keycode, (dst_hid, entry.dst_modifier));
                }
                CAPS_ENTRY_VIRTUAL => {
                    out.caps_actions.insert(entry.src_keycode, entry.vaction_idx);
                }
                CAPS_ENTRY_JUMP_A => {
                    out.caps_jump.insert(entry.src_keycode, 0);
                }
                CAPS_ENTRY_JUMP_B => {
                    out.caps_jump.insert(entry.src_keycode, 1);
                }
                CAPS_ENTRY_SWAP => {
                    out.caps_swap.insert(entry.src_keycode);
                }
                CAPS_ENTRY_CLIP_PULL => {
                    out.caps_clip_pull.insert(entry.src_keycode);
                }
                _ => {}
            }
        }

        out.action_labels = cfg.virtual_actions.iter()
            .filter_map(|a| a.label().map(str::to_owned))
            .collect();

        debug!(
            output_index,
            key_remaps    = out.key_remaps.len(),
            mod_remaps    = out.modifier_remaps.len(),
            caps_chords   = out.caps_chords.len(),
            caps_actions  = out.caps_actions.len(),
            caps_jumps    = out.caps_jump.len(),
            caps_swaps    = out.caps_swap.len(),
            unmapped_pass = out.unmapped_passthrough,
            "compiled output config"
        );
        for (src, (dst, req_mod)) in &out.key_remaps {
            debug!("  key remap: {src:#04x} → {dst:#04x} (req_mod={req_mod:#04x})");
        }
        for (src, dst) in &out.modifier_remaps {
            debug!("  mod remap: {src:#04x} → {dst:#04x}");
        }
        for (src, (dst, dst_mod)) in &out.caps_chords {
            debug!("  caps chord: {src:#04x} → {dst:#04x} (mod={dst_mod:#04x})");
        }
        for (src, slot) in &out.caps_actions {
            let label = out.action_labels.get(*slot as usize).map(String::as_str).unwrap_or("?");
            debug!("  caps action: {src:#04x} → slot {slot} ({label})");
        }
        for (src, target) in &out.caps_jump {
            debug!("  caps jump: {src:#04x} → output {target}");
        }
        for src in &out.caps_swap {
            debug!("  caps swap: {src:#04x}");
        }
        for src in &out.caps_clip_pull {
            debug!("  caps clip-pull: {src:#04x}");
        }

        out
    }
}

// ── Layer state ───────────────────────────────────────────────────────────────

/// Per-keyboard mutable state for the remap engine.
#[derive(Debug, Default)]
pub struct LayerState {
    /// True while the Caps Lock key is physically held.
    pub caps_held: bool,
    /// HID modifier bitmask currently applied (updated on every modifier event).
    pub modifier_bits: u8,
    /// HID keycodes currently reported as pressed.
    pub pressed: HashSet<u8>,
    /// Keys whose down-event was consumed by the caps layer; their up-event
    /// must also be suppressed without re-evaluating the binding.
    consumed_caps_keys: HashSet<u8>,
    /// Whether any caps-layer binding was consumed during the current caps hold.
    /// Determines whether to emit Esc on caps release (tap-caps = Esc).
    caps_consumed: bool,
}

// ── Core processing ───────────────────────────────────────────────────────────

/// Process one key event and return what should be injected.
///
/// `hid`          — USB HID keycode (0x04–0xFF), or 0 for a pure modifier event.
/// `modifier_bit` — non-zero if this event is for a modifier key
///                  (use `keycodes::evdev_modifier_bit`).  When non-zero, `hid`
///                  should be 0.
/// `value`        — VALUE_DOWN / VALUE_UP / VALUE_REPEAT.
/// `cfg`          — compiled config for the currently active output.
/// `state`        — mutable per-keyboard state (lives for the lifetime of one grab).
pub fn process_key(
    hid: u8,
    modifier_bit: u8,
    value: i32,
    cfg: &CompiledOutputConfig,
    state: &mut LayerState,
) -> ProcessResult {
    // ── Modifier event ────────────────────────────────────────────────────────
    if modifier_bit != 0 {
        let effective_bit = *cfg.modifier_remaps.get(&modifier_bit).unwrap_or(&modifier_bit);

        match value {
            VALUE_DOWN | VALUE_REPEAT => state.modifier_bits |= effective_bit,
            VALUE_UP                  => state.modifier_bits &= !effective_bit,
            _                        => {}
        }

        return ProcessResult {
            events: vec![SyntheticKey { hid: 0, modifiers: state.modifier_bits, value }],
            ..Default::default()
        };
    }

    if hid == 0 {
        return ProcessResult::default();
    }

    // ── Caps Lock key itself ───────────────────────────────────────────────────
    // HID 0x39 = Caps Lock.  We suppress the physical key and use it as a
    // layer-shift key.  Tap (no binding consumed) = Esc.
    if hid == 0x39 {
        match value {
            VALUE_DOWN => {
                state.caps_held = true;
                state.caps_consumed = false;
                return ProcessResult::default();
            }
            VALUE_UP => {
                let was_tap = !state.caps_consumed;
                state.caps_held = false;
                state.caps_consumed = false;
                state.consumed_caps_keys.clear();
                if was_tap {
                    // Tap caps → Esc
                    return ProcessResult {
                        events: vec![
                            SyntheticKey { hid: 0x29, modifiers: state.modifier_bits, value: VALUE_DOWN },
                            SyntheticKey { hid: 0x29, modifiers: state.modifier_bits, value: VALUE_UP },
                        ],
                        ..Default::default()
                    };
                }
                return ProcessResult::default();
            }
            _ => return ProcessResult::default(),
        }
    }

    // ── Caps layer active ─────────────────────────────────────────────────────
    if state.caps_held {
        match value {
            VALUE_REPEAT => {
                // Suppress repeats for consumed keys; pass through for unmapped.
                if state.consumed_caps_keys.contains(&hid) {
                    return ProcessResult::default();
                }
                let mapped = apply_key_remap(hid, state.modifier_bits, cfg);
                return ProcessResult {
                    events: vec![SyntheticKey { hid: mapped, modifiers: state.modifier_bits, value: VALUE_REPEAT }],
                    ..Default::default()
                };
            }
            VALUE_UP => {
                // If this key was consumed on the way down, suppress its up too.
                if state.consumed_caps_keys.remove(&hid) {
                    return ProcessResult::default();
                }
                // Unmapped key held during caps — pass through as normal.
                let mapped = apply_key_remap(hid, state.modifier_bits, cfg);
                state.pressed.remove(&mapped);
                return ProcessResult {
                    events: vec![SyntheticKey { hid: mapped, modifiers: state.modifier_bits, value: VALUE_UP }],
                    ..Default::default()
                };
            }
            VALUE_DOWN => {
                // On macOS, key-repeat arrives as repeated VALUE_DOWN, not VALUE_REPEAT.
                // Suppress re-firing for already-consumed keys.
                if state.consumed_caps_keys.contains(&hid) {
                    return ProcessResult::default();
                }
                // Jump to specific output.
                if let Some(&target) = cfg.caps_jump.get(&hid) {
                    state.consumed_caps_keys.insert(hid);
                    state.caps_consumed = true;
                    return ProcessResult { switch_output: Some(target), ..Default::default() };
                }
                // Swap outputs.
                if cfg.caps_swap.contains(&hid) {
                    state.consumed_caps_keys.insert(hid);
                    state.caps_consumed = true;
                    let next = (cfg.output_index + 1) % cfg.output_count.max(1);
                    return ProcessResult { switch_output: Some(next), ..Default::default() };
                }
                // Clipboard pull.
                if cfg.caps_clip_pull.contains(&hid) {
                    state.consumed_caps_keys.insert(hid);
                    state.caps_consumed = true;
                    return ProcessResult { clip_pull: true, ..Default::default() };
                }
                // Fire virtual action.
                if let Some(&slot) = cfg.caps_actions.get(&hid) {
                    state.consumed_caps_keys.insert(hid);
                    state.caps_consumed = true;
                    return ProcessResult { fire_action: Some(slot), ..Default::default() };
                }
                // Chord.
                if let Some(&(dst_hid, dst_mod)) = cfg.caps_chords.get(&hid) {
                    state.consumed_caps_keys.insert(hid);
                    state.caps_consumed = true;
                    // Temporarily drop current modifiers, press chord target, restore.
                    let saved = state.modifier_bits;
                    let mut events = Vec::new();
                    if saved != 0 {
                        events.push(SyntheticKey { hid: 0, modifiers: 0, value: VALUE_DOWN });
                    }
                    events.push(SyntheticKey { hid: dst_hid, modifiers: dst_mod, value: VALUE_DOWN });
                    events.push(SyntheticKey { hid: dst_hid, modifiers: dst_mod, value: VALUE_UP });
                    if saved != 0 {
                        events.push(SyntheticKey { hid: 0, modifiers: saved, value: VALUE_DOWN });
                    }
                    return ProcessResult { events, ..Default::default() };
                }
                // Unmapped caps+key: pass through if unmapped_passthrough is set.
                if cfg.unmapped_passthrough {
                    let mapped = apply_key_remap(hid, state.modifier_bits, cfg);
                    state.pressed.insert(mapped);
                    return ProcessResult {
                        events: vec![SyntheticKey { hid: mapped, modifiers: state.modifier_bits, value: VALUE_DOWN }],
                        ..Default::default()
                    };
                }
                return ProcessResult::default();
            }
            _ => {}
        }
    }

    // ── Normal key remap ──────────────────────────────────────────────────────
    let mapped = apply_key_remap(hid, state.modifier_bits, cfg);

    match value {
        VALUE_DOWN => {
            state.pressed.insert(mapped);
            ProcessResult {
                events: vec![SyntheticKey { hid: mapped, modifiers: state.modifier_bits, value: VALUE_DOWN }],
                ..Default::default()
            }
        }
        VALUE_UP => {
            state.pressed.remove(&mapped);
            ProcessResult {
                events: vec![SyntheticKey { hid: mapped, modifiers: state.modifier_bits, value: VALUE_UP }],
                ..Default::default()
            }
        }
        VALUE_REPEAT => ProcessResult {
            events: vec![SyntheticKey { hid: mapped, modifiers: state.modifier_bits, value: VALUE_REPEAT }],
            ..Default::default()
        },
        _ => ProcessResult::default(),
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn apply_key_remap(hid: u8, current_mods: u8, cfg: &CompiledOutputConfig) -> u8 {
    if let Some(&(dst, required_mod)) = cfg.key_remaps.get(&hid) {
        if required_mod == 0 || (current_mods & required_mod) != 0 {
            return dst;
        }
    }
    hid
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use super::*;
    use crate::config::{
        CapsLayer, CapsLayerEntry, KeyRemap, MachineConfig, ModifierRemap, VirtualAction,
        CAPS_ENTRY_CHORD, CAPS_ENTRY_JUMP_A, CAPS_ENTRY_JUMP_B, CAPS_ENTRY_SWAP, CAPS_ENTRY_VIRTUAL,
    };

    fn make_cfg() -> CompiledOutputConfig {
        let cfg = MachineConfig {
            virtual_actions: {
                let mut v = vec![VirtualAction::Unset; 32];
                v[0] = VirtualAction::AppLaunch {
                    app_id: "com.slack".into(),
                    label: "Slack".into(),
                };
                v
            },
            key_remaps: vec![
                KeyRemap { src_keycode: 0x39, dst_keycode: 0x29, src_modifier: 0, dst_modifier: 0, output_mask: 3, flags: 0 },
            ],
            modifier_remaps: vec![
                ModifierRemap { src: 0x04, dst: 0x40 }, // lalt → ralt
            ],
            caps_layer: CapsLayer {
                unmapped_passthrough: true,
                entries: vec![
                    CapsLayerEntry { // caps+A → E
                        src_keycode: 0x04, entry_type: CAPS_ENTRY_CHORD,
                        output_mask: 3, dst_modifier: 0,
                        dst_keycodes: [0x08, 0, 0, 0], vaction_idx: 0,
                    },
                    CapsLayerEntry { // caps+T → Ctrl+Enter
                        src_keycode: 0x17, entry_type: CAPS_ENTRY_CHORD,
                        output_mask: 3, dst_modifier: 0x01,
                        dst_keycodes: [0x28, 0, 0, 0], vaction_idx: 0,
                    },
                    CapsLayerEntry { // caps+S → action 0 (Slack)
                        src_keycode: 0x16, entry_type: CAPS_ENTRY_VIRTUAL,
                        output_mask: 3, dst_modifier: 0,
                        dst_keycodes: [0; 4], vaction_idx: 0,
                    },
                    CapsLayerEntry { // caps+H → jump to output A
                        src_keycode: 0x0B, entry_type: CAPS_ENTRY_JUMP_A,
                        output_mask: 3, ..CapsLayerEntry::default()
                    },
                    CapsLayerEntry { // caps+K → jump to output B
                        src_keycode: 0x0E, entry_type: CAPS_ENTRY_JUMP_B,
                        output_mask: 3, ..CapsLayerEntry::default()
                    },
                    CapsLayerEntry { // caps+N → swap
                        src_keycode: 0x11, entry_type: CAPS_ENTRY_SWAP,
                        output_mask: 3, ..CapsLayerEntry::default()
                    },
                ],
            },
            skip: vec![],
        };
        CompiledOutputConfig::from_config(&cfg, 0, 2)
    }

    #[test]
    fn passthrough_regular_key() {
        let cfg = make_cfg();
        let mut state = LayerState::default();
        let r = process_key(0x04, 0, VALUE_DOWN, &cfg, &mut state);
        assert_eq!(r.events.len(), 1);
        assert_eq!(r.events[0].hid, 0x04);
        assert_eq!(r.events[0].value, VALUE_DOWN);
    }

    #[test]
    fn caps_suppressed_sets_flag() {
        let cfg = make_cfg();
        let mut state = LayerState::default();
        let r = process_key(0x39, 0, VALUE_DOWN, &cfg, &mut state);
        assert!(r.events.is_empty());
        assert!(state.caps_held);
    }

    #[test]
    fn tap_caps_emits_esc() {
        let cfg = make_cfg();
        let mut state = LayerState::default();
        process_key(0x39, 0, VALUE_DOWN, &cfg, &mut state);
        let r = process_key(0x39, 0, VALUE_UP, &cfg, &mut state);
        // Two events: esc down + esc up
        assert_eq!(r.events.len(), 2);
        assert_eq!(r.events[0].hid, 0x29); // ESC
        assert_eq!(r.events[0].value, VALUE_DOWN);
        assert_eq!(r.events[1].value, VALUE_UP);
        assert!(!state.caps_held);
    }

    #[test]
    fn caps_chord_fires() {
        let cfg = make_cfg();
        let mut state = LayerState { caps_held: true, ..Default::default() };
        let r = process_key(0x04 /*A → chord E*/, 0, VALUE_DOWN, &cfg, &mut state);
        // Events: E down, E up
        let hids: Vec<u8> = r.events.iter().map(|e| e.hid).collect();
        assert!(hids.contains(&0x08), "expected E (0x08) in events, got {hids:?}");
        assert!(state.caps_consumed);
    }

    #[test]
    fn caps_action_fires() {
        let cfg = make_cfg();
        let mut state = LayerState { caps_held: true, ..Default::default() };
        let r = process_key(0x16 /*S*/, 0, VALUE_DOWN, &cfg, &mut state);
        assert!(r.events.is_empty());
        assert_eq!(r.fire_action, Some(0));
    }

    #[test]
    fn caps_jump_a() {
        let cfg = make_cfg();
        let mut state = LayerState { caps_held: true, ..Default::default() };
        let r = process_key(0x0B /*H*/, 0, VALUE_DOWN, &cfg, &mut state);
        assert_eq!(r.switch_output, Some(0));
    }

    #[test]
    fn caps_jump_b() {
        let cfg = make_cfg();
        let mut state = LayerState { caps_held: true, ..Default::default() };
        let r = process_key(0x0E /*K*/, 0, VALUE_DOWN, &cfg, &mut state);
        assert_eq!(r.switch_output, Some(1));
    }

    #[test]
    fn caps_swap_output() {
        // output_index=0, output_count=2 → swap → 1
        let cfg = make_cfg();
        let mut state = LayerState { caps_held: true, ..Default::default() };
        let r = process_key(0x11 /*N*/, 0, VALUE_DOWN, &cfg, &mut state);
        assert_eq!(r.switch_output, Some(1));
    }

    #[test]
    fn modifier_remap_lalt_to_ralt() {
        let cfg = make_cfg();
        let mut state = LayerState::default();
        let r = process_key(0, 0x04 /*lalt*/, VALUE_DOWN, &cfg, &mut state);
        // modifier_remaps[0x04 lalt] = 0x40 ralt
        assert_eq!(state.modifier_bits, 0x40);
        assert_eq!(r.events[0].modifiers, 0x40);
    }

    #[test]
    fn caps_release_suppresses_consumed_key_up() {
        let cfg = make_cfg();
        let mut state = LayerState { caps_held: true, ..Default::default() };
        // Chord down — key consumed
        process_key(0x04, 0, VALUE_DOWN, &cfg, &mut state);
        assert!(state.consumed_caps_keys.contains(&0x04));
        // Key A up — should be suppressed
        let r = process_key(0x04, 0, VALUE_UP, &cfg, &mut state);
        assert!(r.events.is_empty());
    }

    #[test]
    fn value_repeat_passes_through() {
        let cfg = make_cfg();
        let mut state = LayerState::default();
        let r = process_key(0x04, 0, VALUE_REPEAT, &cfg, &mut state);
        assert_eq!(r.events.len(), 1);
        assert_eq!(r.events[0].hid, 0x04);
        assert_eq!(r.events[0].value, VALUE_REPEAT);
    }

    #[test]
    fn key_remap_with_src_modifier_only_fires_when_modifier_held() {
        // caps → esc remap has src_modifier=0 so always fires; test a modifier-gated remap
        let cfg_b = MachineConfig {
            virtual_actions: vec![VirtualAction::Unset; 32],
            key_remaps: vec![
                // Only remap 'a' to 'e' when lshift (0x02) is held
                KeyRemap { src_keycode: 0x04, dst_keycode: 0x08, src_modifier: 0x02,
                           dst_modifier: 0, output_mask: 3, flags: 0 },
            ],
            modifier_remaps: vec![],
            caps_layer: CapsLayer::default(),
            skip: vec![],
        };
        let compiled = CompiledOutputConfig::from_config(&cfg_b, 0, 2);
        let mut state = LayerState::default();

        // Without modifier held → passthrough as 'a'
        let r = process_key(0x04, 0, VALUE_DOWN, &compiled, &mut state);
        assert_eq!(r.events[0].hid, 0x04, "should not remap without required modifier");

        // Hold lshift then press 'a' → remaps to 'e'
        process_key(0, 0x02, VALUE_DOWN, &compiled, &mut state); // lshift down
        let r = process_key(0x04, 0, VALUE_DOWN, &compiled, &mut state);
        assert_eq!(r.events[0].hid, 0x08, "should remap to 'e' when lshift held");
    }

    #[test]
    fn unmapped_passthrough_false_drops_unbound_caps_key() {
        let cfg_b = MachineConfig {
            virtual_actions: vec![VirtualAction::Unset; 32],
            key_remaps: vec![],
            modifier_remaps: vec![],
            caps_layer: CapsLayer {
                unmapped_passthrough: false,
                entries: vec![],
            },
            skip: vec![],
        };
        let compiled = CompiledOutputConfig::from_config(&cfg_b, 0, 2);
        let mut state = LayerState { caps_held: true, ..Default::default() };
        let r = process_key(0x04, 0, VALUE_DOWN, &compiled, &mut state);
        assert!(r.events.is_empty(), "unmapped key should be dropped");
        assert!(r.switch_output.is_none());
        assert!(r.fire_action.is_none());
    }

    #[test]
    fn tap_caps_does_not_emit_esc_when_binding_consumed() {
        let cfg = make_cfg();
        let mut state = LayerState::default();
        // caps down
        process_key(0x39, 0, VALUE_DOWN, &cfg, &mut state);
        // caps+A chord consumed
        process_key(0x04, 0, VALUE_DOWN, &cfg, &mut state);
        assert!(state.caps_consumed, "chord should have set caps_consumed");
        // caps up — should NOT emit Esc because a binding was consumed
        let r = process_key(0x39, 0, VALUE_UP, &cfg, &mut state);
        assert!(r.events.is_empty(), "Esc should not fire when binding was consumed");
    }

    #[test]
    fn chord_with_held_modifier_restores_modifiers() {
        let cfg = make_cfg();
        let mut state = LayerState::default();
        // Hold lctrl (bit 0x01)
        process_key(0, 0x01, VALUE_DOWN, &cfg, &mut state);
        assert_eq!(state.modifier_bits, 0x01);
        // Caps held
        state.caps_held = true;
        // caps+A chord with lctrl held — should clear mods, emit chord, restore mods
        let r = process_key(0x04, 0, VALUE_DOWN, &cfg, &mut state);
        // Events: modifier-clear report, E down, E up, modifier-restore report
        let modifier_reports: Vec<_> = r.events.iter().filter(|e| e.hid == 0).collect();
        assert!(!modifier_reports.is_empty(), "should emit modifier clear/restore events");
        // First modifier event should have modifiers=0 (clear)
        assert_eq!(modifier_reports[0].modifiers, 0, "first report clears modifiers");
        // Last modifier event should restore 0x01
        assert_eq!(modifier_reports.last().unwrap().modifiers, 0x01, "last report restores modifiers");
    }

    #[test]
    fn output_mask_excludes_entry() {
        // Compile cfg as output 1 (B), but caps entry has output_mask=1 (only A)
        let cfg_b = MachineConfig {
            virtual_actions: vec![VirtualAction::Unset; 32],
            key_remaps: vec![],
            modifier_remaps: vec![],
            caps_layer: CapsLayer {
                unmapped_passthrough: false,
                entries: vec![
                    CapsLayerEntry {
                        src_keycode: 0x04, entry_type: CAPS_ENTRY_CHORD,
                        output_mask: 1, // only output A (bit 0)
                        dst_modifier: 0, dst_keycodes: [0x08, 0, 0, 0], vaction_idx: 0,
                    },
                ],
            },
            skip: vec![],
        };
        let compiled = CompiledOutputConfig::from_config(&cfg_b, 1 /*output B*/, 2);
        let mut state = LayerState { caps_held: true, ..Default::default() };
        // With output_mask=1 and output_index=1, bit check (1 & (1<<1)) = 0 → not compiled
        let r = process_key(0x04, 0, VALUE_DOWN, &compiled, &mut state);
        // unmapped_passthrough=false → consumed silently
        assert!(r.events.is_empty());
        assert!(r.fire_action.is_none());
    }
}
