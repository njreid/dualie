//! keycodes.rs — USB HID keycode helpers for the caps-lock chord daemon.

/// HID usage code for the Caps Lock key (USB HID Usage Page 0x07).
pub const CAPS_LOCK_HID: u8 = 0x39;

/// Resolve a config key name (single ASCII letter/digit, or a named key like
/// "left"/"f1"/"enter") to its USB HID keycode (Usage Page 0x07).
pub fn keycode_by_name(name: &str) -> Option<u8> {
    if name.len() == 1 {
        let c = name.chars().next().unwrap();
        if c.is_ascii_lowercase() {
            return Some(0x04 + (c as u8 - b'a'));
        }
        if let Some(kc) = match c {
            '0' => Some(0x27u8),
            '1'..='9' => Some(0x1E + (c as u8 - b'1')),
            _ => None,
        } {
            return Some(kc);
        }
    }

    match name.to_ascii_lowercase().as_str() {
        "enter" | "return" => Some(0x28),
        "esc" | "escape" => Some(0x29),
        "backspace" => Some(0x2A),
        "tab" => Some(0x2B),
        "space" => Some(0x2C),
        "minus" | "-" => Some(0x2D),
        "equals" | "=" => Some(0x2E),
        "lbracket" | "[" => Some(0x2F),
        "rbracket" | "]" => Some(0x30),
        "backslash" | "\\" => Some(0x31),
        "semicolon" | ";" => Some(0x33),
        "quote" | "'" => Some(0x34),
        "grave" | "`" => Some(0x35),
        "comma" | "," => Some(0x36),
        "period" | "." => Some(0x37),
        "slash" | "/" => Some(0x38),
        "capslock" => Some(0x39),
        "f1" => Some(0x3A), "f2" => Some(0x3B), "f3" => Some(0x3C),
        "f4" => Some(0x3D), "f5" => Some(0x3E), "f6" => Some(0x3F),
        "f7" => Some(0x40), "f8" => Some(0x41), "f9" => Some(0x42),
        "f10" => Some(0x43), "f11" => Some(0x44), "f12" => Some(0x45),
        "printscreen" => Some(0x46),
        "scrolllock" => Some(0x47),
        "pause" => Some(0x48),
        "insert" => Some(0x49),
        "home" => Some(0x4A),
        "pageup" => Some(0x4B),
        "delete" | "del" => Some(0x4C),
        "end" => Some(0x4D),
        "pagedown" => Some(0x4E),
        "right" => Some(0x4F),
        "left" => Some(0x50),
        "down" => Some(0x51),
        "up" => Some(0x52),
        "mute" => Some(0x7F),
        "volup" | "volumeup" => Some(0x80),
        "voldown" | "volumedown" => Some(0x81),
        _ => None,
    }
}

/// Return the HID modifier bitmask bit for a HID keycode in the modifier
/// range (0xE0-0xE7, Usage Page 0x07). Returns 0 if the keycode is not a
/// modifier. Used by the macOS IOHIDManager path, which delivers raw HID
/// usage codes.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
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
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_and_digits() {
        assert_eq!(keycode_by_name("a"), Some(0x04));
        assert_eq!(keycode_by_name("z"), Some(0x1D));
        assert_eq!(keycode_by_name("1"), Some(0x1E));
        assert_eq!(keycode_by_name("0"), Some(0x27));
    }

    #[test]
    fn named_keys() {
        assert_eq!(keycode_by_name("left"), Some(0x50));
        assert_eq!(keycode_by_name("right"), Some(0x4F));
        assert_eq!(keycode_by_name("up"), Some(0x52));
        assert_eq!(keycode_by_name("down"), Some(0x51));
        assert_eq!(keycode_by_name("f12"), Some(0x45));
        assert_eq!(keycode_by_name("ESC"), Some(0x29)); // case-insensitive
    }

    #[test]
    fn unknown_name_returns_none() {
        assert_eq!(keycode_by_name("notakey"), None);
        assert_eq!(keycode_by_name(""), None);
    }

    #[test]
    fn caps_lock_constant_matches_table() {
        assert_eq!(CAPS_LOCK_HID, 0x39);
        assert_eq!(keycode_by_name("capslock"), Some(CAPS_LOCK_HID));
    }

    #[test]
    fn modifier_bits() {
        assert_eq!(hid_modifier_bit(0xE0), 0x01); // Left Control
        assert_eq!(hid_modifier_bit(0xE1), 0x02); // Left Shift
        assert_eq!(hid_modifier_bit(0xE7), 0x80); // Right Meta
        assert_eq!(hid_modifier_bit(0x04), 0);    // 'a' is not a modifier
    }
}
