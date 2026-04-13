/// intercept/keycodes.rs — evdev ↔ USB HID keycode translation.
///
/// evdev key codes are defined in linux/input-event-codes.h (KEY_* constants).
/// HID keycodes are from USB HID Usage Page 0x07 (Keyboard/Keypad).
///
/// Modifier keys are handled separately: the evdev modifier key codes map to
/// HID modifier bitmask bits (used in the first byte of a boot-protocol report).

// ── evdev → HID ───────────────────────────────────────────────────────────────

/// Translate an evdev `KEY_*` code to a USB HID keycode (Usage Page 0x07).
/// Returns 0 if the key has no HID equivalent or is a modifier key
/// (modifiers are handled via `evdev_modifier_bit`).
pub fn evdev_to_hid(evdev: u16) -> u8 {
    // Modifier keys return 0 here — use evdev_modifier_bit() instead.
    match evdev {
        // Letters A–Z
        30 => 0x04, // KEY_A
        48 => 0x05, // KEY_B
        46 => 0x06, // KEY_C
        32 => 0x07, // KEY_D
        18 => 0x08, // KEY_E
        33 => 0x09, // KEY_F
        34 => 0x0A, // KEY_G
        35 => 0x0B, // KEY_H
        23 => 0x0C, // KEY_I
        36 => 0x0D, // KEY_J
        37 => 0x0E, // KEY_K
        38 => 0x0F, // KEY_L
        50 => 0x10, // KEY_M
        49 => 0x11, // KEY_N
        24 => 0x12, // KEY_O
        25 => 0x13, // KEY_P
        16 => 0x14, // KEY_Q
        19 => 0x15, // KEY_R
        31 => 0x16, // KEY_S
        20 => 0x17, // KEY_T
        22 => 0x18, // KEY_U
        47 => 0x19, // KEY_V
        17 => 0x1A, // KEY_W
        45 => 0x1B, // KEY_X
        21 => 0x1C, // KEY_Y
        44 => 0x1D, // KEY_Z

        // Digits 1–0
        2  => 0x1E, // KEY_1
        3  => 0x1F, // KEY_2
        4  => 0x20, // KEY_3
        5  => 0x21, // KEY_4
        6  => 0x22, // KEY_5
        7  => 0x23, // KEY_6
        8  => 0x24, // KEY_7
        9  => 0x25, // KEY_8
        10 => 0x26, // KEY_9
        11 => 0x27, // KEY_0

        // Control cluster
        28 => 0x28, // KEY_ENTER
        1  => 0x29, // KEY_ESC
        14 => 0x2A, // KEY_BACKSPACE
        15 => 0x2B, // KEY_TAB
        57 => 0x2C, // KEY_SPACE
        12 => 0x2D, // KEY_MINUS
        13 => 0x2E, // KEY_EQUAL
        26 => 0x2F, // KEY_LEFTBRACE
        27 => 0x30, // KEY_RIGHTBRACE
        43 => 0x31, // KEY_BACKSLASH
        39 => 0x33, // KEY_SEMICOLON
        40 => 0x34, // KEY_APOSTROPHE
        41 => 0x35, // KEY_GRAVE
        51 => 0x36, // KEY_COMMA
        52 => 0x37, // KEY_DOT
        53 => 0x38, // KEY_SLASH
        58 => 0x39, // KEY_CAPSLOCK

        // F-keys
        59 => 0x3A, // KEY_F1
        60 => 0x3B, // KEY_F2
        61 => 0x3C, // KEY_F3
        62 => 0x3D, // KEY_F4
        63 => 0x3E, // KEY_F5
        64 => 0x3F, // KEY_F6
        65 => 0x40, // KEY_F7
        66 => 0x41, // KEY_F8
        67 => 0x42, // KEY_F9
        68 => 0x43, // KEY_F10
        87 => 0x44, // KEY_F11
        88 => 0x45, // KEY_F12

        // Navigation / editing
        110 => 0x49, // KEY_INSERT
        111 => 0x4C, // KEY_DELETE
        102 => 0x4A, // KEY_HOME
        107 => 0x4D, // KEY_END
        104 => 0x4B, // KEY_PAGEUP
        109 => 0x4E, // KEY_PAGEDOWN
        105 => 0x50, // KEY_LEFT
        106 => 0x4F, // KEY_RIGHT
        103 => 0x52, // KEY_UP
        108 => 0x51, // KEY_DOWN

        // Keypad
        69  => 0x53, // KEY_NUMLOCK
        98  => 0x54, // KEY_KPSLASH
        55  => 0x55, // KEY_KPASTERISK
        74  => 0x56, // KEY_KPMINUS
        78  => 0x57, // KEY_KPPLUS
        96  => 0x58, // KEY_KPENTER
        79  => 0x59, // KEY_KP1
        80  => 0x5A, // KEY_KP2
        81  => 0x5B, // KEY_KP3
        75  => 0x5C, // KEY_KP4
        76  => 0x5D, // KEY_KP5
        77  => 0x5E, // KEY_KP6
        71  => 0x5F, // KEY_KP7
        72  => 0x60, // KEY_KP8
        73  => 0x61, // KEY_KP9
        82  => 0x62, // KEY_KP0
        83  => 0x63, // KEY_KPDOT

        // Media / extra
        113 => 0x7F, // KEY_MUTE
        114 => 0x81, // KEY_VOLUMEDOWN
        115 => 0x80, // KEY_VOLUMEUP
        164 => 0xCD, // KEY_PLAYPAUSE
        165 => 0xB6, // KEY_PREVIOUSSONG
        163 => 0xB5, // KEY_NEXTSONG

        // Modifier keys — return 0, handled via evdev_modifier_bit()
        29 | 97  => 0, // KEY_LEFTCTRL / KEY_RIGHTCTRL
        42 | 54  => 0, // KEY_LEFTSHIFT / KEY_RIGHTSHIFT
        56 | 100 => 0, // KEY_LEFTALT / KEY_RIGHTALT
        125 | 126 => 0, // KEY_LEFTMETA / KEY_RIGHTMETA

        _ => 0,
    }
}

/// Translate a USB HID keycode back to an evdev `KEY_*` code.
/// Returns 0 if not found.
pub fn hid_to_evdev(hid: u8) -> u16 {
    match hid {
        0x04 => 30, 0x05 => 48, 0x06 => 46, 0x07 => 32, 0x08 => 18,
        0x09 => 33, 0x0A => 34, 0x0B => 35, 0x0C => 23, 0x0D => 36,
        0x0E => 37, 0x0F => 38, 0x10 => 50, 0x11 => 49, 0x12 => 24,
        0x13 => 25, 0x14 => 16, 0x15 => 19, 0x16 => 31, 0x17 => 20,
        0x18 => 22, 0x19 => 47, 0x1A => 17, 0x1B => 45, 0x1C => 21,
        0x1D => 44,
        0x1E => 2,  0x1F => 3,  0x20 => 4,  0x21 => 5,  0x22 => 6,
        0x23 => 7,  0x24 => 8,  0x25 => 9,  0x26 => 10, 0x27 => 11,
        0x28 => 28, 0x29 => 1,  0x2A => 14, 0x2B => 15, 0x2C => 57,
        0x2D => 12, 0x2E => 13, 0x2F => 26, 0x30 => 27, 0x31 => 43,
        0x33 => 39, 0x34 => 40, 0x35 => 41, 0x36 => 51, 0x37 => 52,
        0x38 => 53, 0x39 => 58,
        0x3A => 59, 0x3B => 60, 0x3C => 61, 0x3D => 62, 0x3E => 63,
        0x3F => 64, 0x40 => 65, 0x41 => 66, 0x42 => 67, 0x43 => 68,
        0x44 => 87, 0x45 => 88,
        0x49 => 110, 0x4A => 102, 0x4B => 104, 0x4C => 111,
        0x4D => 107, 0x4E => 109, 0x4F => 106, 0x50 => 105,
        0x51 => 108, 0x52 => 103,
        0x53 => 69,  0x54 => 98,  0x55 => 55,  0x56 => 74,  0x57 => 78,
        0x58 => 96,  0x59 => 79,  0x5A => 80,  0x5B => 81,  0x5C => 75,
        0x5D => 76,  0x5E => 77,  0x5F => 71,  0x60 => 72,  0x61 => 73,
        0x62 => 82,  0x63 => 83,
        0x7F => 113, 0x80 => 115, 0x81 => 114,
        // Modifier HID keycodes (0xE0–0xE7) → evdev modifier key codes
        0xE0 => 29,  // lctrl
        0xE1 => 42,  // lshift
        0xE2 => 56,  // lalt
        0xE3 => 125, // lmeta
        0xE4 => 97,  // rctrl
        0xE5 => 54,  // rshift
        0xE6 => 100, // ralt
        0xE7 => 126, // rmeta
        _ => 0,
    }
}

// ── Modifier bit mapping ──────────────────────────────────────────────────────

/// Return the HID modifier bitmask bit for an evdev modifier key code,
/// or 0 if the key is not a modifier.
///
/// Bit layout (matches USB HID boot-protocol modifier byte):
///   0x01 lctrl  0x02 lshift  0x04 lalt  0x08 lmeta
///   0x10 rctrl  0x20 rshift  0x40 ralt  0x80 rmeta
pub fn evdev_modifier_bit(evdev: u16) -> u8 {
    match evdev {
        29  => 0x01, // KEY_LEFTCTRL
        42  => 0x02, // KEY_LEFTSHIFT
        56  => 0x04, // KEY_LEFTALT
        125 => 0x08, // KEY_LEFTMETA
        97  => 0x10, // KEY_RIGHTCTRL
        54  => 0x20, // KEY_RIGHTSHIFT
        100 => 0x40, // KEY_RIGHTALT
        126 => 0x80, // KEY_RIGHTMETA
        _   => 0,
    }
}

/// Return the evdev key code for a given HID modifier bitmask bit.
/// Uses the left-hand variant for each modifier.
pub fn modifier_bit_to_evdev(bit: u8) -> u16 {
    match bit {
        0x01 => 29,  // lctrl
        0x02 => 42,  // lshift
        0x04 => 56,  // lalt
        0x08 => 125, // lmeta
        0x10 => 97,  // rctrl
        0x20 => 54,  // rshift
        0x40 => 100, // ralt
        0x80 => 126, // rmeta
        _    => 0,
    }
}

// ── HID modifier keys ─────────────────────────────────────────────────────────

/// Return the modifier bitmask bit for a USB HID keycode in the modifier range
/// (0xE0–0xE7, Usage Page 0x07).  Returns 0 if the keycode is not a modifier.
///
/// Used by the macOS IOHIDManager path, which delivers raw HID usage codes.
pub fn hid_modifier_bit(hid: u8) -> u8 {
    match hid {
        0xE0 => 0x01, // Left Control
        0xE1 => 0x02, // Left Shift
        0xE2 => 0x04, // Left Alt
        0xE3 => 0x08, // Left Meta (Command)
        0xE4 => 0x10, // Right Control
        0xE5 => 0x20, // Right Shift
        0xE6 => 0x40, // Right Alt
        0xE7 => 0x80, // Right Meta (Command)
        _    => 0,
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_letters() {
        for evdev in [30u16, 48, 46, 32, 18, 33, 34, 35, 23, 36,
                      37, 38, 50, 49, 24, 25, 16, 19, 31, 20,
                      22, 47, 17, 45, 21, 44] {
            let hid = evdev_to_hid(evdev);
            assert_ne!(hid, 0, "evdev {evdev} should map to non-zero HID");
            assert_eq!(hid_to_evdev(hid), evdev, "roundtrip failed for evdev {evdev}");
        }
    }

    #[test]
    fn modifier_keys_return_zero_from_evdev_to_hid() {
        assert_eq!(evdev_to_hid(29), 0);  // KEY_LEFTCTRL
        assert_eq!(evdev_to_hid(42), 0);  // KEY_LEFTSHIFT
    }

    #[test]
    fn modifier_bits_roundtrip() {
        for (evdev, bit) in [(29u16, 0x01u8), (42, 0x02), (56, 0x04), (125, 0x08),
                              (97, 0x10), (54, 0x20), (100, 0x40), (126, 0x80)] {
            assert_eq!(evdev_modifier_bit(evdev), bit);
            assert_eq!(modifier_bit_to_evdev(bit), evdev);
        }
    }

    #[test]
    fn roundtrip_digits_and_nav() {
        // Digits 1-0
        for evdev in [2u16, 3, 4, 5, 6, 7, 8, 9, 10, 11] {
            let hid = evdev_to_hid(evdev);
            assert_ne!(hid, 0, "digit evdev {evdev} should map to non-zero HID");
            assert_eq!(hid_to_evdev(hid), evdev);
        }
        // Navigation
        for evdev in [105u16 /*left*/, 106 /*right*/, 103 /*up*/, 108 /*down*/,
                      102 /*home*/, 107 /*end*/, 110 /*insert*/, 111 /*delete*/] {
            let hid = evdev_to_hid(evdev);
            assert_ne!(hid, 0, "nav evdev {evdev} should map to non-zero HID");
            assert_eq!(hid_to_evdev(hid), evdev);
        }
        // F-keys F1-F12
        for evdev in [59u16, 60, 61, 62, 63, 64, 65, 66, 67, 68, 87, 88] {
            let hid = evdev_to_hid(evdev);
            assert_ne!(hid, 0, "F-key evdev {evdev} should map to non-zero HID");
            assert_eq!(hid_to_evdev(hid), evdev);
        }
    }

    #[test]
    fn unknown_codes_return_zero() {
        assert_eq!(evdev_to_hid(0), 0);      // KEY_RESERVED
        assert_eq!(evdev_to_hid(999), 0);    // way out of range
        assert_eq!(hid_to_evdev(0), 0);      // no HID 0
        assert_eq!(hid_to_evdev(0x32), 0);   // unmapped HID
        assert_eq!(evdev_modifier_bit(0), 0);
        assert_eq!(evdev_modifier_bit(1), 0); // KEY_ESC is not a modifier
    }
}
