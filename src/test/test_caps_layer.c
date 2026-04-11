/*
 * Self-contained test for the caps-layer logic.
 *
 * The structs and process_caps_layer() function are copied verbatim from
 * src/include/structs.h and src/keyboard.c so that no pico-sdk headers are
 * needed.  If the firmware logic changes, update the copy here as well.
 */

#include <stdio.h>
#include <stdlib.h>
#include "stubs.h"

#define ASSERT(cond) \
    do { \
        if (!(cond)) { \
            fprintf(stderr, "FAIL %s:%d: %s\n", __FILE__, __LINE__, #cond); \
            exit(1); \
        } \
    } while (0)

/* ── Inline struct definitions (mirrors src/include/structs.h) ───────────── */

#define CAPS_LAYER_MAX_ENTRIES 32
#define CAPS_ENTRY_CHORD   0
#define CAPS_ENTRY_VIRTUAL 1

typedef struct {
    uint8_t src_keycode;
    uint8_t entry_type;
    uint8_t output_mask;
    uint8_t dst_modifier;
    union {
        uint8_t dst_keycodes[4];
        struct {
            uint8_t vaction_idx;
            uint8_t _vpad[3];
        };
    };
} caps_layer_entry_t;

typedef struct {
    uint8_t            unmapped_passthrough;
    uint8_t            _pad[3];
    caps_layer_entry_t entries[CAPS_LAYER_MAX_ENTRIES];
} caps_layer_t;

/* Minimal device_t for the caps-layer tests */
typedef struct {
    uint8_t      active_output;
    bool         caps_lock_on;
    caps_layer_t caps_layer[2];
} device_t;

/* ── Helpers copied from keyboard.c ─────────────────────────────────────── */

static bool key_in_report(uint8_t key, const hid_keyboard_report_t *report) {
    for (int j = 0; j < KEYS_IN_USB_REPORT; j++) {
        if (key == report->keycode[j])
            return true;
    }
    return false;
}

/* Stub: no USB queue available; record the toggle side-effect only */
static void queue_kbd_report(hid_keyboard_report_t *r, device_t *state) {
    (void)r; (void)state;
}

static void caps_send_lock_toggle(device_t *state) {
    hid_keyboard_report_t press   = {0};
    hid_keyboard_report_t release = {0};
    press.keycode[0] = HID_KEY_CAPS_LOCK;
    queue_kbd_report(&press,   state);
    queue_kbd_report(&release, state);
    state->caps_lock_on = !state->caps_lock_on;
}

/* Stub for VIRTUAL entry type – minimal vkey table (F13-F24 = 0x68-0x73) */
static uint8_t dualie_vkey(uint8_t idx) {
    static const uint8_t table[] = {
        0x68, 0x69, 0x6A, 0x6B, 0x6C, 0x6D,
        0x6E, 0x6F, 0x70, 0x71, 0x72, 0x73,
    };
    if (idx < (uint8_t)(sizeof(table) / sizeof(table[0])))
        return table[idx];
    return 0;
}

/* Copy of process_caps_layer from src/keyboard.c */
static bool process_caps_layer(hid_keyboard_report_t *report, device_t *state) {
    const uint8_t output      = state->active_output;
    const caps_layer_t *layer = &state->caps_layer[output];
    const uint8_t output_bit  = 1u << output;

    /* Step 1: strip CapsLock */
    for (int j = 0; j < KEYS_IN_USB_REPORT; j++) {
        if (report->keycode[j] == HID_KEY_CAPS_LOCK)
            report->keycode[j] = 0;
    }

    /* Step 2: Caps+Esc → toggle OS CapsLock, consume */
    if (key_in_report(HID_KEY_ESCAPE, report)) {
        caps_send_lock_toggle(state);
        return true;
    }

    /* Nothing else pressed → consume silently */
    bool any_key = false;
    for (int j = 0; j < KEYS_IN_USB_REPORT; j++) {
        if (report->keycode[j]) { any_key = true; break; }
    }
    if (!any_key && !report->modifier)
        return true;

    /* Step 3: transform each keycode */
    bool chord_applied = false;
    for (int j = 0; j < KEYS_IN_USB_REPORT; j++) {
        uint8_t src = report->keycode[j];
        if (!src) continue;

        bool matched = false;
        for (int i = 0; i < CAPS_LAYER_MAX_ENTRIES; i++) {
            const caps_layer_entry_t *e = &layer->entries[i];
            if (!e->src_keycode)        continue;
            if (e->src_keycode != src)  continue;
            if (!(e->output_mask & output_bit)) continue;

            if (e->entry_type == CAPS_ENTRY_CHORD) {
                if (!chord_applied) {
                    memset(report, 0, sizeof(*report));
                    report->modifier = e->dst_modifier;
                    for (int k = 0; k < 4; k++) {
                        if (e->dst_keycodes[k])
                            report->keycode[k] = e->dst_keycodes[k];
                    }
                    chord_applied = true;
                }
            } else if (e->entry_type == CAPS_ENTRY_VIRTUAL) {
                uint8_t vk = dualie_vkey(e->vaction_idx);
                if (vk) report->keycode[j] = vk;
            }

            matched = true;
            break;
        }

        if (!matched) {
            if (!layer->unmapped_passthrough)
                report->keycode[j] = 0;
        }
    }

    return false;
}

/* ── Test cases ──────────────────────────────────────────────────────────── */

void test_caps_esc_consumed(void) {
    device_t state = {0};
    state.active_output = 0;
    state.caps_layer[0].unmapped_passthrough = 1;

    hid_keyboard_report_t report = {0};
    report.keycode[0] = HID_KEY_ESCAPE;

    bool consumed = process_caps_layer(&report, &state);
    ASSERT(consumed == true);
}

void test_chord_entry(void) {
    device_t state = {0};
    state.active_output = 0;
    state.caps_layer[0].unmapped_passthrough = 1;

    /* Configure entry: src=0x04 → modifier=0x01, keycodes=[0x16,0,0,0] */
    caps_layer_entry_t *e = &state.caps_layer[0].entries[0];
    e->src_keycode     = 0x04;
    e->entry_type      = CAPS_ENTRY_CHORD;
    e->output_mask     = 1; /* output A only */
    e->dst_modifier    = 0x01;
    e->dst_keycodes[0] = 0x16;
    e->dst_keycodes[1] = 0x00;
    e->dst_keycodes[2] = 0x00;
    e->dst_keycodes[3] = 0x00;

    hid_keyboard_report_t report = {0};
    report.keycode[0] = 0x04;

    bool consumed = process_caps_layer(&report, &state);
    ASSERT(consumed == false);
    ASSERT(report.modifier == 0x01);
    ASSERT(report.keycode[0] == 0x16);
}

void test_unmapped_swallowed(void) {
    device_t state = {0};
    state.active_output = 0;
    state.caps_layer[0].unmapped_passthrough = 0; /* swallow unmapped */

    hid_keyboard_report_t report = {0};
    report.keycode[0] = 0x07; /* some unmapped key */

    bool consumed = process_caps_layer(&report, &state);
    ASSERT(consumed == false);
    ASSERT(report.keycode[0] == 0); /* swallowed */
}

void test_unmapped_passthrough(void) {
    device_t state = {0};
    state.active_output = 0;
    state.caps_layer[0].unmapped_passthrough = 1; /* pass through */

    hid_keyboard_report_t report = {0};
    report.keycode[0] = 0x07; /* some unmapped key */

    bool consumed = process_caps_layer(&report, &state);
    ASSERT(consumed == false);
    ASSERT(report.keycode[0] == 0x07); /* unchanged */
}
