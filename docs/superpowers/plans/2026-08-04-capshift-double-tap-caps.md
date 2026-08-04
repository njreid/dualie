# Capshift Double-Tap Caps Lock Toggle Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Double-tapping Caps Lock (two bare presses within 300ms, nothing else pressed during either hold) toggles the real macOS Caps Lock state, by forwarding the second tap's own key-down/key-up as a real Caps Lock keystroke through the virtual keyboard.

**Architecture:** `chord.rs` stays OS/FFI-free and unit-testable. `ChordState::process` gains a `now: std::time::Instant` parameter; `hid.rs` (which already touches OS/FFI) supplies `Instant::now()` on every HID event it processes. New `ChordState` fields track whether the current Caps Lock hold is still a "bare tap" candidate and whether a pending double-tap is in progress.

**Tech Stack:** Rust, `std::time::{Instant, Duration}` only (no new dependencies).

## Global Constraints

- Double-tap window is a fixed 300ms, not configurable via config.kdl.
- A tap only counts if Caps Lock was pressed and released with no other key pressed during that hold. Any other key-down while Caps Lock is held cancels that hold's tap eligibility and clears any pending double-tap sequence.
- Feature is on by default — no config.kdl changes required to enable it.
- After a double-tap resolves, the sequence resets (a third rapid tap starts counting fresh, does not chain into another toggle).
- Design doc: `docs/superpowers/specs/2026-08-04-capshift-double-tap-caps-design.md`.

---

### Task 1: Add double-tap detection to `ChordState`

**Files:**
- Modify: `capshift/src/chord.rs`

**Interfaces:**
- Consumes: nothing new — this task only changes `chord.rs` internals and its own public signature.
- Produces: `ChordState::process(&mut self, hid: u8, down: bool, now: std::time::Instant) -> KeyOutcome` — the `now` parameter is new; existing callers (`hid.rs`) must be updated in Task 2. `KeyOutcome` variants are unchanged (double-tap resolution reuses the existing `KeyOutcome::Forward(u8)` variant with `target == caps_lock_hid`).

- [ ] **Step 1: Write the failing/not-yet-compiling tests**

Replace the entire `#[cfg(test)] mod tests { ... }` block at the bottom of `capshift/src/chord.rs` with the version below. This updates every existing test call site to pass a `now: Instant` argument (using a shared `base()` helper so timings are simple to reason about) and adds five new tests for double-tap behavior.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

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

    /// A fixed base instant tests build relative timings from, so test
    /// intent ("50ms later", "400ms later") is explicit at each call site.
    fn base() -> Instant {
        Instant::now()
    }

    #[test]
    fn caps_alone_is_swallowed() {
        let mut chord = ChordState::new(CAPS, bindings());
        let t = base();
        assert_eq!(chord.process(CAPS, true, t), KeyOutcome::Swallow);
        assert_eq!(chord.process(CAPS, false, t), KeyOutcome::Swallow);
    }

    #[test]
    fn unbound_key_without_caps_passes_through() {
        let mut chord = ChordState::new(CAPS, bindings());
        let t = base();
        assert_eq!(chord.process(X, true, t), KeyOutcome::Passthrough);
        assert_eq!(chord.process(X, false, t), KeyOutcome::Passthrough);
    }

    #[test]
    fn unbound_key_while_caps_held_passes_through() {
        let mut chord = ChordState::new(CAPS, bindings());
        let t = base();
        chord.process(CAPS, true, t);
        assert_eq!(chord.process(X, true, t), KeyOutcome::Passthrough);
    }

    #[test]
    fn remap_binding_forwards_target_keycode() {
        let mut chord = ChordState::new(CAPS, bindings());
        let t = base();
        chord.process(CAPS, true, t);
        assert_eq!(chord.process(H, true, t), KeyOutcome::Forward(LEFT));
        assert_eq!(chord.process(H, false, t), KeyOutcome::Forward(LEFT));
    }

    #[test]
    fn remap_key_up_still_translates_after_caps_released_first() {
        // caps down, h down (forwarded), caps up, h up — the h key-up must
        // still translate to LEFT so the virtual key doesn't get stuck down.
        let mut chord = ChordState::new(CAPS, bindings());
        let t = base();
        chord.process(CAPS, true, t);
        assert_eq!(chord.process(H, true, t), KeyOutcome::Forward(LEFT));
        assert_eq!(chord.process(CAPS, false, t), KeyOutcome::Swallow);
        assert_eq!(chord.process(H, false, t), KeyOutcome::Forward(LEFT));
    }

    #[test]
    fn action_binding_fires_and_swallows_its_key_up() {
        let mut chord = ChordState::new(CAPS, bindings());
        let t = base();
        chord.process(CAPS, true, t);
        assert_eq!(chord.process(S, true, t), KeyOutcome::Fire(slack()));
        assert_eq!(chord.process(S, false, t), KeyOutcome::Swallow);
    }

    #[test]
    fn key_not_matched_while_chord_active_does_not_leave_stray_state() {
        let mut chord = ChordState::new(CAPS, bindings());
        let t = base();
        chord.process(CAPS, true, t);
        assert_eq!(chord.process(X, true, t), KeyOutcome::Passthrough);
        assert_eq!(chord.process(X, false, t), KeyOutcome::Passthrough);
    }

    #[test]
    fn set_bindings_does_not_disrupt_in_flight_remap() {
        let mut chord = ChordState::new(CAPS, bindings());
        let t = base();
        chord.process(CAPS, true, t);
        assert_eq!(chord.process(H, true, t), KeyOutcome::Forward(LEFT));

        // Config reloads mid-chord with a completely different binding table
        // (H is no longer bound to anything).
        chord.set_bindings(HashMap::new());

        // The key-up for H must still translate to LEFT — the physical key
        // is still "in flight" from before the reload.
        assert_eq!(chord.process(H, false, t), KeyOutcome::Forward(LEFT));
    }

    #[test]
    fn double_tap_within_window_forwards_real_caps_lock() {
        let mut chord = ChordState::new(CAPS, bindings());
        let t = base();

        // Tap 1: bare press-release.
        assert_eq!(chord.process(CAPS, true, t), KeyOutcome::Swallow);
        assert_eq!(chord.process(CAPS, false, t), KeyOutcome::Swallow);

        // Tap 2, 50ms later: within the 300ms window, forwards as a real
        // Caps Lock keystroke instead of being swallowed.
        let t2 = t + Duration::from_millis(50);
        assert_eq!(chord.process(CAPS, true, t2), KeyOutcome::Forward(CAPS));
        assert_eq!(chord.process(CAPS, false, t2), KeyOutcome::Forward(CAPS));
    }

    #[test]
    fn taps_more_than_300ms_apart_do_not_double_tap() {
        let mut chord = ChordState::new(CAPS, bindings());
        let t = base();

        assert_eq!(chord.process(CAPS, true, t), KeyOutcome::Swallow);
        assert_eq!(chord.process(CAPS, false, t), KeyOutcome::Swallow);

        let t2 = t + Duration::from_millis(301);
        assert_eq!(chord.process(CAPS, true, t2), KeyOutcome::Swallow);
        assert_eq!(chord.process(CAPS, false, t2), KeyOutcome::Swallow);
    }

    #[test]
    fn chord_use_is_not_a_tap_and_cancels_pending_sequence() {
        let mut chord = ChordState::new(CAPS, bindings());
        let t = base();

        // First hold is used as a chord modifier (caps+h), not a bare tap.
        chord.process(CAPS, true, t);
        assert_eq!(chord.process(H, true, t), KeyOutcome::Forward(LEFT));
        assert_eq!(chord.process(H, false, t), KeyOutcome::Forward(LEFT));
        assert_eq!(chord.process(CAPS, false, t), KeyOutcome::Swallow);

        // A quick second bare tap right after does NOT count as completing
        // a double-tap, since the first hold wasn't a bare tap.
        let t2 = t + Duration::from_millis(50);
        assert_eq!(chord.process(CAPS, true, t2), KeyOutcome::Swallow);
        assert_eq!(chord.process(CAPS, false, t2), KeyOutcome::Swallow);
    }

    #[test]
    fn chord_use_between_two_bare_taps_cancels_the_sequence() {
        let mut chord = ChordState::new(CAPS, bindings());
        let t = base();

        // Bare tap 1.
        chord.process(CAPS, true, t);
        chord.process(CAPS, false, t);

        // Chord use in between, 50ms later.
        let t2 = t + Duration::from_millis(50);
        chord.process(CAPS, true, t2);
        chord.process(H, true, t2);
        chord.process(H, false, t2);
        chord.process(CAPS, false, t2);

        // Bare tap again, 50ms after that (100ms after tap 1, still within
        // the window measured from tap 1 — but the chord use must have
        // cancelled tap 1, so this is not a double-tap).
        let t3 = t2 + Duration::from_millis(50);
        assert_eq!(chord.process(CAPS, true, t3), KeyOutcome::Swallow);
        assert_eq!(chord.process(CAPS, false, t3), KeyOutcome::Swallow);
    }

    #[test]
    fn resolved_double_tap_does_not_chain_into_a_third_tap() {
        let mut chord = ChordState::new(CAPS, bindings());
        let t = base();

        // Tap 1 (bare) + tap 2 (resolves as double-tap).
        chord.process(CAPS, true, t);
        chord.process(CAPS, false, t);
        let t2 = t + Duration::from_millis(50);
        assert_eq!(chord.process(CAPS, true, t2), KeyOutcome::Forward(CAPS));
        assert_eq!(chord.process(CAPS, false, t2), KeyOutcome::Forward(CAPS));

        // Tap 3, 50ms after tap 2: must NOT also forward — the sequence
        // reset after tap 2 resolved, so tap 3 is just a fresh bare tap.
        let t3 = t2 + Duration::from_millis(50);
        assert_eq!(chord.process(CAPS, true, t3), KeyOutcome::Swallow);
        assert_eq!(chord.process(CAPS, false, t3), KeyOutcome::Swallow);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail to compile**

Run: `cargo test --manifest-path capshift/Cargo.toml chord::`

Expected: compile error — `process` takes 2 arguments but 3 were supplied (the implementation hasn't been updated yet).

- [ ] **Step 3: Implement double-tap detection**

In `capshift/src/chord.rs`:

1. Add this import near the top of the file (below the existing `use std::collections::{HashMap, HashSet};`):

```rust
use std::time::{Duration, Instant};
```

2. Add a module-level constant, placed just above `pub struct ChordState`:

```rust
/// Two bare Caps Lock taps within this window count as a double-tap.
const DOUBLE_TAP_WINDOW: Duration = Duration::from_millis(300);
```

3. Replace the `ChordState` struct definition with:

```rust
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
    /// Set the moment any other key-down occurs during the current Caps
    /// Lock hold, disqualifying this hold from being a "bare tap".
    chord_used: bool,
    /// Set when Caps Lock releases after a hold with nothing else pressed
    /// (a "bare tap"). Checked on the next Caps Lock down to detect a
    /// double-tap; cleared by a chord use or once a double-tap resolves.
    last_bare_tap_up: Option<Instant>,
    /// Set when the current Caps Lock hold's key-down was forwarded as a
    /// real Caps Lock keystroke (the second tap of a double-tap), so the
    /// matching key-up is forwarded too instead of swallowed.
    forwarding_caps: bool,
}
```

4. Replace `ChordState::new` with:

```rust
    pub fn new(caps_lock_hid: u8, bindings: HashMap<u8, Binding>) -> Self {
        Self {
            caps_lock_hid,
            bindings,
            active: false,
            remapped: HashMap::new(),
            fired: HashSet::new(),
            chord_used: false,
            last_bare_tap_up: None,
            forwarding_caps: false,
        }
    }
```

5. Replace the body of `process` with:

```rust
    pub fn process(&mut self, hid: u8, down: bool, now: Instant) -> KeyOutcome {
        if hid == self.caps_lock_hid {
            self.active = down;

            if down {
                self.chord_used = false;

                let is_double_tap = self
                    .last_bare_tap_up
                    .is_some_and(|prev_up| now.duration_since(prev_up) <= DOUBLE_TAP_WINDOW);

                self.last_bare_tap_up = None;

                if is_double_tap {
                    self.forwarding_caps = true;
                    return KeyOutcome::Forward(self.caps_lock_hid);
                }
                return KeyOutcome::Swallow;
            }

            // Caps Lock key-up.
            if self.forwarding_caps {
                self.forwarding_caps = false;
                return KeyOutcome::Forward(self.caps_lock_hid);
            }
            if !self.chord_used {
                self.last_bare_tap_up = Some(now);
            }
            return KeyOutcome::Swallow;
        }

        if down && self.active {
            // Using Caps Lock as a chord modifier disqualifies this hold
            // from being a "bare tap" and cancels any pending sequence.
            self.chord_used = true;
            self.last_bare_tap_up = None;
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
```

Leave `set_bindings` unchanged.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --manifest-path capshift/Cargo.toml chord::`

Expected: all tests in `chord::tests` pass (8 pre-existing + 5 new = 13 tests total), 0 failures.

- [ ] **Step 5: Commit**

```bash
git add capshift/src/chord.rs
git commit -m "feat(capshift): detect double-tap caps lock in chord state machine"
```

---

### Task 2: Wire real time into the HID event loop

**Files:**
- Modify: `capshift/src/hid.rs:165`

**Interfaces:**
- Consumes: `ChordState::process(&mut self, hid: u8, down: bool, now: std::time::Instant) -> KeyOutcome` from Task 1.
- Produces: nothing new for later tasks — this is the last task in the plan.

- [ ] **Step 1: Update the call site**

In `capshift/src/hid.rs`, change:

```rust
        match state.chord.process(usage, down) {
```

to:

```rust
        match state.chord.process(usage, down, std::time::Instant::now()) {
```

- [ ] **Step 2: Type-check against the macOS target**

This crate's `hid` module is `#[cfg(target_os = "macos")]`-gated, so it only compiles when targeting macOS. From the repo root (not the `capshift/` subdirectory), run:

```bash
cargo check --manifest-path capshift/Cargo.toml --target aarch64-apple-darwin
```

(If the `aarch64-apple-darwin` std target isn't installed, run `rustup target add aarch64-apple-darwin` first.)

Expected: `Finished` with no errors.

- [ ] **Step 3: Run the full test suite one more time**

Run: `cargo test --manifest-path capshift/Cargo.toml`

Expected: all tests pass (this re-runs Task 1's `chord::` tests plus any other existing tests in the crate; `hid`/`kvhd` modules are excluded on this host since they're macOS-only).

- [ ] **Step 4: Commit**

```bash
git add capshift/src/hid.rs
git commit -m "feat(capshift): forward real time into chord state for double-tap detection"
```
