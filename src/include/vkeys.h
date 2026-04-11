/*
 * Virtual keycode table for Dualie caps-layer actions.
 *
 * When a caps-layer entry of type VIRTUAL fires, the firmware substitutes
 * the source keycode with one of the 32 "virtual action" keycodes defined
 * here.  The daemon intercepts these keycodes system-wide and dispatches
 * the configured action (app launch, etc.) before suppressing the event.
 *
 * Slot assignment (stable across firmware versions):
 *   0–11   F13–F24        (HID 0x68–0x73)  – universally supported
 *  12–19   Execute…Cut    (HID 0x74–0x7B)  – well-supported on Linux/Mac
 *  20–27   Intl1–Intl8    (HID 0x87–0x8E)  – JIS/ISO extra keys, rarely used
 *  28–31   Lang1–Lang4    (HID 0x90–0x93)  – Korean/CJK toggle keys
 *
 * The daemon uses the same DUALIE_VKEY_TABLE to map events back to slot
 * indices, so the two sides stay in sync automatically.
 */
#pragma once

#include <stdint.h>

#define DUALIE_VKEY_COUNT 32

/* HID Usage Page 0x07 (Keyboard/Keypad) codes used as virtual action keys. */
static const uint8_t DUALIE_VKEY_TABLE[DUALIE_VKEY_COUNT] = {
    /* Slots 0–11: F13–F24 */
    0x68, 0x69, 0x6A, 0x6B, 0x6C, 0x6D,
    0x6E, 0x6F, 0x70, 0x71, 0x72, 0x73,
    /* Slots 12–19: Execute, Help, Menu, Select, Stop, Again, Undo, Cut */
    0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7A, 0x7B,
    /* Slots 20–27: International1–8 */
    0x87, 0x88, 0x89, 0x8A, 0x8B, 0x8C, 0x8D, 0x8E,
    /* Slots 28–31: Lang1–Lang4 */
    0x90, 0x91, 0x92, 0x93,
};

/* Return the virtual HID keycode for action slot idx, or 0 if out of range. */
static inline uint8_t dualie_vkey(uint8_t idx) {
    if (idx >= DUALIE_VKEY_COUNT)
        return 0;
    return DUALIE_VKEY_TABLE[idx];
}

/* Return the action slot index for a given HID keycode, or -1 if not a vkey. */
static inline int8_t dualie_vkey_slot(uint8_t keycode) {
    for (uint8_t i = 0; i < DUALIE_VKEY_COUNT; i++) {
        if (DUALIE_VKEY_TABLE[i] == keycode)
            return (int8_t)i;
    }
    return -1;
}
