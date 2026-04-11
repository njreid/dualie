/// Maps rdev `Key` variants → dualie vaction slot (0-31), or `None`.
///
/// Our 32 virtual key slots correspond to:
///   Slots  0-11 → F13-F24          (HID 0x68-0x73)
///   Slots 12-19 → Execute-Cut      (HID 0x74-0x7B)
///   Slots 20-27 → International1-8 (HID 0x87-0x8E)
///   Slots 28-31 → Lang1-4          (HID 0x90-0x93)
///
/// rdev only names F1-F12 as enum variants; everything above F12 arrives as
/// `Key::Unknown(platform_scancode)`.  The platform scan codes differ between
/// macOS (CGEventTap virtual key codes) and Linux (evdev codes), so we
/// compile-in the right table per target.
use rdev::Key;

pub fn rdev_key_to_vslot(key: &Key) -> Option<usize> {
    match key {
        Key::Unknown(code) => unknown_to_vslot(*code),
        _ => None,
    }
}

// ── Linux evdev scan codes ────────────────────────────────────────────────────

#[cfg(target_os = "linux")]
fn unknown_to_vslot(code: u32) -> Option<usize> {
    // Source: linux/input-event-codes.h
    match code {
        // F13-F24 (slots 0-11) — KEY_F13=183 … KEY_F24=194
        183 => Some(0),
        184 => Some(1),
        185 => Some(2),
        186 => Some(3),
        187 => Some(4),
        188 => Some(5),
        189 => Some(6),
        190 => Some(7),
        191 => Some(8),
        192 => Some(9),
        193 => Some(10),
        194 => Some(11),

        // Execute-Cut range (slots 12-19) — no standard Linux evdev equivalents
        // for HID 0x74-0x7B; the Pico firmware can be configured to use the
        // F13-F24 range exclusively if execute/cut slots are not needed.

        // International keys (slots 20-27) — KEY_RO, KEY_KATAKANA, etc.
        0x59 => Some(20), // KEY_RO
        0x5C => Some(21), // KEY_KATAKANA
        0x7D => Some(22), // KEY_YEN
        0x5D => Some(23), // KEY_HENKAN
        0x5E => Some(24), // KEY_HIRAGANA
        0x5F => Some(25), // KEY_KATAKANAHIRAGANA
        0xE3 => Some(26), // KEY_ZENKAKUHANKAKU

        // Lang keys (slots 28-31) — KEY_LANG1-4
        0x122 => Some(28),
        0x123 => Some(29),
        0x124 => Some(30),
        0x125 => Some(31),

        _ => None,
    }
}

// ── macOS CGEventTap virtual key codes ────────────────────────────────────────

#[cfg(target_os = "macos")]
fn unknown_to_vslot(code: u32) -> Option<usize> {
    // Source: HIToolbox/Events.h (kVK_* constants)
    match code {
        // F13-F24 (slots 0-11)
        105 => Some(0),  // kVK_F13  0x69
        107 => Some(1),  // kVK_F14  0x6B
        113 => Some(2),  // kVK_F15  0x71
        106 => Some(3),  // kVK_F16  0x6A
        64  => Some(4),  // kVK_F17  0x40
        79  => Some(5),  // kVK_F18  0x4F
        80  => Some(6),  // kVK_F19  0x50
        90  => Some(7),  // kVK_F20  0x5A
        // F21-F24 not standard on macOS hardware; slots 8-11 unused there

        // International keys (slots 20-27) — JIS keyboard virtual key codes
        0x5E => Some(20), // kVK_JIS_Yen
        0x5F => Some(21), // kVK_JIS_Underscore
        0x5D => Some(22), // kVK_JIS_KeypadComma
        0x62 => Some(23), // kVK_JIS_Eisu
        0x68 => Some(24), // kVK_JIS_Kana

        _ => None,
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn unknown_to_vslot(_code: u32) -> Option<usize> {
    None
}
