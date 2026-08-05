//! chord.rs — pure caps-lock chord state machine.
//!
//! Caps Lock is the only chord modifier. While held, the next matching key
//! either fires an app-launch/shell-command action or is translated to a
//! different HID keycode (a "remap" binding, e.g. caps+h -> Left arrow).
//! Everything else passes through unchanged. This module has no OS/FFI
//! dependency so it can be unit tested with synthetic HID keycodes.

use std::collections::{HashMap, HashSet};

/// HID usage emitted by the key labeled Delete on Apple keyboards
/// (Backspace in the USB HID usage table).
const APPLE_DELETE_HID: u8 = 0x2A;

/// An action fired when a chord's binding matches.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    AppLaunch { app_id: String, label: String },
    ShellCommand { command: String, label: String },
}

/// What a bound key does when pressed as part of a caps-lock chord.
#[derive(Debug, Clone, PartialEq)]
pub enum Binding {
    Action(Action),
    /// Remap to a different HID keycode (e.g. caps+h -> Left arrow).
    Remap(u8),
}

/// What the caller should do with a given key event.
#[derive(Debug, Clone, PartialEq)]
pub enum KeyOutcome {
    /// Do not forward this event (it's the Caps Lock key itself).
    Swallow,
    /// Forward the original HID keycode unchanged.
    Passthrough,
    /// Forward a different HID keycode instead of the original.
    Forward(u8),
    /// Fire this action; do not forward the original keycode.
    Fire(Action),
}

/// Tracks whether the caps-lock chord is currently held, and the flat
/// src-HID-keycode -> Binding table.
pub struct ChordState {
    caps_lock_hid: u8,
    bindings: HashMap<u8, Binding>,
    active: bool,
    /// Src HID keycode -> HID keycode actually forwarded, for keys currently
    /// down as part of a matched remap binding. Ensures the key-up event is
    /// translated to match the key-down we already sent, even if Caps Lock
    /// was released in between.
    remapped: HashMap<u8, u8>,
    /// Src HID keycodes currently down as part of a matched action binding,
    /// so their key-up is swallowed instead of leaking through as a stray
    /// keypress.
    fired: HashSet<u8>,
}

impl ChordState {
    pub fn new(caps_lock_hid: u8, bindings: HashMap<u8, Binding>) -> Self {
        Self {
            caps_lock_hid,
            bindings,
            active: false,
            remapped: HashMap::new(),
            fired: HashSet::new(),
        }
    }

    /// Update the binding table in place, preserving in-flight chord state
    /// (active/remapped/fired) so a config reload never loses track of a
    /// physical key the user is still holding down mid-chord.
    pub fn set_bindings(&mut self, bindings: HashMap<u8, Binding>) {
        self.bindings = bindings;
    }

    /// Process one HID key event.
    ///
    /// `hid`  — HID keycode (Usage Page 0x07) of the key.
    /// `down` — true for key-down, false for key-up.
    pub fn process(&mut self, hid: u8, down: bool) -> KeyOutcome {
        if hid == self.caps_lock_hid {
            self.active = down;
            return KeyOutcome::Swallow;
        }

        if !down {
            if let Some(target) = self.remapped.remove(&hid) {
                return KeyOutcome::Forward(target);
            }
            if self.fired.remove(&hid) {
                return KeyOutcome::Swallow;
            }
            return KeyOutcome::Passthrough;
        }

        if self.active {
            // Caps+physical Delete emits one real Caps Lock stroke. A bare
            // Caps press remains swallowed unconditionally.
            if hid == APPLE_DELETE_HID {
                self.remapped.insert(hid, self.caps_lock_hid);
                return KeyOutcome::Forward(self.caps_lock_hid);
            }

            if let Some(binding) = self.bindings.get(&hid) {
                return match binding {
                    Binding::Action(action) => {
                        self.fired.insert(hid);
                        KeyOutcome::Fire(action.clone())
                    }
                    Binding::Remap(target) => {
                        self.remapped.insert(hid, *target);
                        KeyOutcome::Forward(*target)
                    }
                };
            }
        }

        KeyOutcome::Passthrough
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CAPS: u8 = 0x39;
    const H: u8 = 0x0B;
    const LEFT: u8 = 0x50;
    const S: u8 = 0x16;
    const X: u8 = 0x1B;

    fn slack() -> Action {
        Action::AppLaunch { app_id: "com.tinyspeck.slackmacgap".into(), label: "Slack".into() }
    }

    fn bindings() -> HashMap<u8, Binding> {
        let mut m = HashMap::new();
        m.insert(H, Binding::Remap(LEFT));
        m.insert(S, Binding::Action(slack()));
        m
    }

    #[test]
    fn caps_alone_is_swallowed() {
        let mut chord = ChordState::new(CAPS, bindings());
        assert_eq!(chord.process(CAPS, true), KeyOutcome::Swallow);
        assert_eq!(chord.process(CAPS, false), KeyOutcome::Swallow);
    }

    #[test]
    fn unbound_key_without_caps_passes_through() {
        let mut chord = ChordState::new(CAPS, bindings());
        assert_eq!(chord.process(X, true), KeyOutcome::Passthrough);
        assert_eq!(chord.process(X, false), KeyOutcome::Passthrough);
    }

    #[test]
    fn unbound_key_while_caps_held_passes_through() {
        let mut chord = ChordState::new(CAPS, bindings());
        chord.process(CAPS, true);
        assert_eq!(chord.process(X, true), KeyOutcome::Passthrough);
    }

    #[test]
    fn remap_binding_forwards_target_keycode() {
        let mut chord = ChordState::new(CAPS, bindings());
        chord.process(CAPS, true);
        assert_eq!(chord.process(H, true), KeyOutcome::Forward(LEFT));
        assert_eq!(chord.process(H, false), KeyOutcome::Forward(LEFT));
    }

    #[test]
    fn remap_key_up_still_translates_after_caps_released_first() {
        // caps down, h down (forwarded), caps up, h up — the h key-up must
        // still translate to LEFT so the virtual key doesn't get stuck down.
        let mut chord = ChordState::new(CAPS, bindings());
        chord.process(CAPS, true);
        assert_eq!(chord.process(H, true), KeyOutcome::Forward(LEFT));
        assert_eq!(chord.process(CAPS, false), KeyOutcome::Swallow);
        assert_eq!(chord.process(H, false), KeyOutcome::Forward(LEFT));
    }

    #[test]
    fn action_binding_fires_and_swallows_its_key_up() {
        let mut chord = ChordState::new(CAPS, bindings());
        chord.process(CAPS, true);
        assert_eq!(chord.process(S, true), KeyOutcome::Fire(slack()));
        assert_eq!(chord.process(S, false), KeyOutcome::Swallow);
    }

    #[test]
    fn key_not_matched_while_chord_active_does_not_leave_stray_state() {
        let mut chord = ChordState::new(CAPS, bindings());
        chord.process(CAPS, true);
        assert_eq!(chord.process(X, true), KeyOutcome::Passthrough);
        assert_eq!(chord.process(X, false), KeyOutcome::Passthrough);
    }

    #[test]
    fn set_bindings_does_not_disrupt_in_flight_remap() {
        let mut chord = ChordState::new(CAPS, bindings());
        chord.process(CAPS, true);
        assert_eq!(chord.process(H, true), KeyOutcome::Forward(LEFT));

        // Config reloads mid-chord with a completely different binding table
        // (H is no longer bound to anything).
        chord.set_bindings(HashMap::new());

        // The key-up for H must still translate to LEFT — the physical key
        // is still "in flight" from before the reload.
        assert_eq!(chord.process(H, false), KeyOutcome::Forward(LEFT));
    }

    #[test]
    fn caps_delete_forwards_one_caps_lock_stroke() {
        let mut chord = ChordState::new(CAPS, bindings());
        assert_eq!(chord.process(CAPS, true), KeyOutcome::Swallow);
        assert_eq!(chord.process(APPLE_DELETE_HID, true), KeyOutcome::Forward(CAPS));
        assert_eq!(chord.process(APPLE_DELETE_HID, false), KeyOutcome::Forward(CAPS));
        assert_eq!(chord.process(CAPS, false), KeyOutcome::Swallow);
    }

    #[test]
    fn delete_without_caps_remains_backspace() {
        let mut chord = ChordState::new(CAPS, bindings());
        assert_eq!(chord.process(APPLE_DELETE_HID, true), KeyOutcome::Passthrough);
        assert_eq!(chord.process(APPLE_DELETE_HID, false), KeyOutcome::Passthrough);
    }
}
