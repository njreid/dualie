# capshift: double-tap Caps Lock to toggle Caps Lock

## Problem

capshift consumes Caps Lock entirely as its chord modifier — it's never
re-injected as a regular keypress, so the normal Caps Lock toggle behavior
is unavailable while capshift is running (documented as a known limitation
in the README). This adds it back via a double-tap gesture, without giving
up Caps Lock as the chord modifier for everything else.

## Behavior

- A **bare tap** is a Caps Lock press-release with no other key pressed
  during the hold. Using Caps Lock as a chord modifier (any other key-down
  while it's held) is never a tap, bare or otherwise, and cancels any
  pending double-tap sequence.
- Two bare taps within **300ms** (fixed, not configurable) of each other
  count as a double-tap.
- On the second tap's key-down, instead of being swallowed, that
  keystroke is forwarded as a real Caps Lock key-down (and the matching
  key-up as a real Caps Lock key-up) through the virtual keyboard — the
  physical second tap becomes the actual toggle keystroke. macOS handles
  the actual LED/state toggle itself.
- After a resolved double-tap, the sequence resets — a third rapid tap
  starts counting fresh rather than chaining into another toggle.
- On by default; no config.kdl changes needed.

## Architecture

`chord.rs` stays OS/FFI-free and unit-testable. `ChordState::process`
gains a `now: std::time::Instant` parameter, supplied by `hid.rs` (which
already calls into OS/FFI) via `Instant::now()` on every HID event.
Tests construct synthetic timings with a base `Instant::now()` plus
`Duration::from_millis(..)` offsets — no OS dependency introduced.

New `ChordState` fields:
- `last_bare_tap_up: Option<Instant>` — set when Caps releases after a
  hold with nothing else pressed; cleared when a chord fires/remaps, or
  after a double-tap resolves.
- `chord_used: bool` — set the moment any other key-down occurs while
  Caps is held; checked on Caps-up to decide whether to record a bare
  tap.
- `forwarding_caps: bool` — set when the second tap's key-down is
  detected as completing a double-tap, so the matching key-up also
  forwards instead of swallowing.

`process()` logic for `hid == caps_lock_hid`:
- **down**: set `active = true`, `chord_used = false` for this hold. If
  `last_bare_tap_up` is `Some(t)` and `now - t <= 300ms`, this is the
  second tap: clear `last_bare_tap_up`, set `forwarding_caps = true`,
  return `Forward(CAPS_LOCK_HID)`. Otherwise return `Swallow`.
- **up**: set `active = false`. If `forwarding_caps` is set, clear it and
  return `Forward(CAPS_LOCK_HID)`. Otherwise, if `!chord_used`, record
  `last_bare_tap_up = Some(now)`; return `Swallow` either way.

For any other key-down while `active`, in addition to existing
chord-matching logic, set `chord_used = true` and clear
`last_bare_tap_up` (a chord use invalidates any pending tap sequence).

No changes to `hid.rs`'s outcome dispatch — `Forward(target)` already
calls the existing `forward()` path that sends a keycode through the
Karabiner virtual keyboard; `CAPS_LOCK_HID` is just another target
keycode from that path's point of view.

## Error handling

No new failure modes — this is pure in-memory state transition logic
with no I/O. Clock going backwards is not a concern (`Instant` is
monotonic).

## Testing

Unit tests in `chord.rs` (same style as existing tests), using a base
`Instant::now()` and `Duration` offsets:
- Two bare taps within 300ms forward Caps Lock down/up on the second tap.
- Two bare taps more than 300ms apart do not trigger forwarding (both
  swallowed).
- A chord use (caps+h) followed by a quick second tap does not trigger
  double-tap (the chord use isn't a tap, and cancels any prior pending
  tap).
- A resolved double-tap followed immediately by a third tap does not
  chain into forwarding the third tap (sequence resets).
- Existing chord/remap/action tests continue to pass unchanged (aside
  from the new `now` parameter threaded through call sites).
