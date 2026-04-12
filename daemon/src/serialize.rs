/// Serialise `DualieConfig` into the `config_t` binary blob expected by the
/// Dualie firmware (RP2040, little-endian, GCC struct layout).
///
/// Layout derived from `src/include/structs.h` + `src/include/screen.h`.
/// A `static_assert(sizeof(config_t) == CONFIG_T_SIZE)` in the firmware
/// catches any drift between these two files.
///
/// # config_t layout (848 bytes total)
/// ```text
/// offset   size  field
///      0      4  magic_header
///      4      4  version
///      8      6  flags (force_mouse_boot_mode … enforce_ports)
///     14      2  jump_threshold
///     16    112  output[2] (56 bytes each)
///    128      4  _reserved
///    132    192  key_remaps[32] (6 bytes each)
///    324    520  caps_layer[2] (260 bytes each)
///    844      4  checksum
/// ```
///
/// output_t layout (56 bytes):
/// ```text
///  0  4  number
///  4  4  screen_count
///  8  4  screen_index
/// 12  4  speed_x
/// 16  4  speed_y
/// 20  8  border {top(4), bottom(4)}
/// 28  3  os, pos, mouse_park_pos
/// 31  1  padding
/// 32 24  screensaver {mode(1), only_if_inactive(1), pad(6), idle(8), max(8)}
/// ```
///
/// caps_layer_t layout (260 bytes):
/// ```text
///  0  1  unmapped_passthrough
///  1  3  _pad
///  4 256  entries[32] (8 bytes each):
///           src_keycode(1) entry_type(1) output_mask(1) dst_modifier(1)
///           union[4] (dst_keycodes[4] or vaction_idx+pad[3])
/// ```

use crate::config::{
    CapsLayerEntry, DualieConfig, KeyRemap,
    CAPS_ENTRY_CHORD, CAPS_ENTRY_VIRTUAL, CAPS_LAYER_MAX,
};

/// Must match REMAP_FLAG_MOD_ONLY in src/include/config.h
const REMAP_FLAG_MOD_ONLY: u8 = 0x01;

// Firmware constants that must match config.h / vkeys.h
const FIRMWARE_MAGIC:   u32 = 0x00B00B1E5;  // deskhop magic_header value
const CONFIG_VERSION:   u32 = 10;
const MAX_KEY_REMAPS:   usize = 32;

// Absolute struct size — verified by firmware static_assert
pub const CONFIG_T_SIZE: usize = 848;

/// Produce the raw `config_t` bytes for the firmware.
/// Only the dualie-specific fields (key_remaps, caps_layer) are set;
/// all deskhop-native fields (mouse speed, borders, screensaver…) are
/// zeroed — the firmware merges this blob with its own defaults on flash.
pub fn config_to_bytes(cfg: &DualieConfig) -> Vec<u8> {
    let mut buf = vec![0u8; CONFIG_T_SIZE];

    // ── Header ────────────────────────────────────────────────────────────────
    write_u32(&mut buf, 0,   FIRMWARE_MAGIC);
    write_u32(&mut buf, 4,   CONFIG_VERSION);
    // flags at 8-13: leave as 0 (firmware uses its own stored values)
    // jump_threshold at 14-15: leave as 0
    // output[2] at 16-127: leave as 0 (firmware uses its own stored values)
    // _reserved at 128: 0

    // ── Key remaps (offset 132, 32 × 6 bytes) ─────────────────────────────────
    // Merge per-output remap lists into the single firmware flat table.
    // Identical remaps (same src/dst/mods/flags) on both outputs get mask=3.
    let merged = merge_key_remaps(cfg);
    for (i, r) in merged.iter().enumerate().take(MAX_KEY_REMAPS) {
        let off = 132 + i * 6;
        buf[off]     = r.src_keycode;
        buf[off + 1] = r.dst_keycode;
        buf[off + 2] = r.src_modifier;
        buf[off + 3] = r.dst_modifier;
        buf[off + 4] = r.output_mask;
        buf[off + 5] = r.flags;
    }

    // ── Caps layers (offset 324, 2 × 260 bytes) ───────────────────────────────
    for (out_idx, output) in cfg.outputs.iter().enumerate() {
        let layer_off = 324 + out_idx * 260;
        buf[layer_off] = output.caps_layer.unmapped_passthrough as u8;
        // _pad[3] already 0

        for (e_idx, entry) in output.caps_layer.entries.iter()
            .take(CAPS_LAYER_MAX)
            .enumerate()
        {
            write_caps_entry(&mut buf, layer_off + 4 + e_idx * 8, entry);
        }
    }

    // ── Checksum (offset 844) ─────────────────────────────────────────────────
    // deskhop save_config() does: uint8_t checksum = calc_crc32(...) which
    // truncates the CRC-32 to its lowest byte, then stores it in the uint32_t
    // checksum field.  Reproduce that here.
    let csum = crc32(&buf[..844]) as u8;
    write_u32(&mut buf, 844, csum as u32);

    buf
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn write_u32(buf: &mut [u8], offset: usize, val: u32) {
    buf[offset..offset + 4].copy_from_slice(&val.to_le_bytes());
}

fn write_caps_entry(buf: &mut [u8], offset: usize, e: &CapsLayerEntry) {
    buf[offset]     = e.src_keycode;
    buf[offset + 1] = e.entry_type;
    buf[offset + 2] = e.output_mask;
    buf[offset + 3] = e.dst_modifier;
    if e.entry_type == CAPS_ENTRY_CHORD {
        buf[offset + 4..offset + 8].copy_from_slice(&e.dst_keycodes);
    } else if e.entry_type == CAPS_ENTRY_VIRTUAL {
        // vaction_idx stored directly — firmware sends VirtualAction { slot }
        // over CDC-ACM serial instead of emitting an F13-F24 HID keycode.
        buf[offset + 4] = e.vaction_idx;
        buf[offset + 5] = 0;
        buf[offset + 6] = 0;
        buf[offset + 7] = 0;
    }
}

/// Merge per-output key remaps and modifier remaps into one flat list (≤ 32 entries).
/// Identical entries on both outputs get output_mask = 3 (both).
fn merge_key_remaps(cfg: &DualieConfig) -> Vec<KeyRemap> {
    let mut out: Vec<KeyRemap> = Vec::new();

    for (out_idx, output) in cfg.outputs.iter().enumerate() {
        let mask = 1u8 << out_idx;

        // ── Regular key remaps ────────────────────────────────────────────
        for r in &output.key_remaps {
            if r.src_keycode == 0 { continue; }
            if let Some(existing) = out.iter_mut().find(|e| {
                e.src_keycode == r.src_keycode
                    && e.dst_keycode == r.dst_keycode
                    && e.src_modifier == r.src_modifier
                    && e.dst_modifier == r.dst_modifier
                    && e.flags == r.flags
            }) {
                existing.output_mask |= mask;
            } else {
                out.push(KeyRemap { output_mask: mask, ..r.clone() });
            }
        }

        // ── Modifier-only remaps ──────────────────────────────────────────
        for m in &output.modifier_remaps {
            if m.src == 0 { continue; }
            if let Some(existing) = out.iter_mut().find(|e| {
                e.flags == REMAP_FLAG_MOD_ONLY
                    && e.src_modifier == m.src
                    && e.dst_modifier == m.dst
            }) {
                existing.output_mask |= mask;
            } else {
                out.push(KeyRemap {
                    src_keycode:  0,
                    dst_keycode:  0,
                    src_modifier: m.src,
                    dst_modifier: m.dst,
                    output_mask:  mask,
                    flags:        REMAP_FLAG_MOD_ONLY,
                });
            }
        }
    }

    out.truncate(MAX_KEY_REMAPS);
    out
}

/// Simple CRC-32 (IEEE 802.3) — matches the firmware's `calc_checksum`.
/// The firmware uses a simple XOR-based checksum; if it's something else,
/// adjust here.  Currently mirrors deskhop's verify_checksum() approach.
pub(crate) fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        crc ^= (byte as u32) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 {
                (crc << 1) ^ 0x04C1_1DB7
            } else {
                crc << 1
            };
        }
    }
    !crc
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_blob_is_correct_size() {
        let cfg = DualieConfig::default();
        let bytes = config_to_bytes(&cfg);
        assert_eq!(bytes.len(), CONFIG_T_SIZE, "config_t size mismatch");
    }

    #[test]
    fn magic_and_version_are_written() {
        let bytes = config_to_bytes(&DualieConfig::default());
        assert_eq!(u32::from_le_bytes(bytes[0..4].try_into().unwrap()), FIRMWARE_MAGIC);
        assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()), CONFIG_VERSION);
    }

    #[test]
    fn key_remap_at_correct_offset() {
        use crate::config::{KeyRemap, DualieConfig};

        let remap = KeyRemap {
            src_keycode:  0x04,
            dst_keycode:  0x05,
            src_modifier: 0x00,
            dst_modifier: 0xFF,
            output_mask:  1,
            flags:        0,
        };
        let mut cfg = DualieConfig::default();
        cfg.outputs[0].key_remaps = vec![remap];

        let bytes = config_to_bytes(&cfg);

        // First remap entry starts at offset 132 (6 bytes each)
        assert_eq!(bytes[132], 0x04, "src_keycode");
        assert_eq!(bytes[133], 0x05, "dst_keycode");
        assert_eq!(bytes[134], 0x00, "src_modifier");
        assert_eq!(bytes[135], 0xFF, "dst_modifier");
        assert_eq!(bytes[136], 1,    "output_mask");
        assert_eq!(bytes[137], 0,    "flags");
    }

    #[test]
    fn caps_layer_at_correct_offset() {
        use crate::config::{CapsLayerEntry, CAPS_ENTRY_CHORD, DualieConfig};

        let entry = CapsLayerEntry {
            src_keycode:  0x10,
            entry_type:   CAPS_ENTRY_CHORD,
            output_mask:  3,
            dst_modifier: 0x02,
            dst_keycodes: [0x04, 0x00, 0x00, 0x00],
            vaction_idx:  0,
        };
        let mut cfg = DualieConfig::default();
        cfg.outputs[0].caps_layer.entries = vec![entry];

        let bytes = config_to_bytes(&cfg);

        // caps_layer[0] starts at 324; +1 byte unmapped_passthrough +3 pad = +4
        // first entry at 328
        let off = 328;
        assert_eq!(bytes[off],     0x10, "src_keycode");
        assert_eq!(bytes[off + 1], CAPS_ENTRY_CHORD, "entry_type");
        assert_eq!(bytes[off + 2], 3,    "output_mask");
        assert_eq!(bytes[off + 3], 0x02, "dst_modifier");
        assert_eq!(bytes[off + 4], 0x04, "dst_keycodes[0]");
        assert_eq!(bytes[off + 5], 0x00, "dst_keycodes[1]");
        assert_eq!(bytes[off + 6], 0x00, "dst_keycodes[2]");
        assert_eq!(bytes[off + 7], 0x00, "dst_keycodes[3]");
    }

    #[test]
    fn checksum_is_truncated_crc32() {
        let cfg   = DualieConfig::default();
        let bytes = config_to_bytes(&cfg);

        // Recompute CRC-32 of first 844 bytes independently
        let expected_crc = crc32(&bytes[..844]);
        let expected_byte = expected_crc as u8;

        assert_eq!(bytes[844], expected_byte, "checksum low byte");
        // Remaining 3 bytes of the uint32 field must be zero
        assert_eq!(&bytes[845..848], &[0u8, 0, 0], "checksum high bytes");
    }

    #[test]
    fn merge_key_remaps_deduplication() {
        use crate::config::{KeyRemap, DualieConfig};

        let make_remap = |output_mask: u8| KeyRemap {
            src_keycode:  4,
            dst_keycode:  5,
            src_modifier: 0,
            dst_modifier: 0,
            output_mask,
            flags: 0,
        };

        // Same remap in both outputs → merged with output_mask = 3
        let mut cfg = DualieConfig::default();
        cfg.outputs[0].key_remaps = vec![make_remap(1)];
        cfg.outputs[1].key_remaps = vec![make_remap(2)];

        let merged = merge_key_remaps(&cfg);
        assert_eq!(merged.len(), 1, "should deduplicate to 1 entry");
        assert_eq!(merged[0].output_mask, 3, "merged output_mask should be 3");

        // Remap only in output 1 → output_mask = 2
        let mut cfg2 = DualieConfig::default();
        cfg2.outputs[1].key_remaps = vec![make_remap(2)];

        let merged2 = merge_key_remaps(&cfg2);
        assert_eq!(merged2.len(), 1);
        assert_eq!(merged2[0].output_mask, 2, "output 1 only should have mask 2");
    }
}
