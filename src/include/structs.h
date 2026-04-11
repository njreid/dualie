/*
 * This file is part of DeskHop (https://github.com/hrvach/deskhop).
 * Copyright (c) 2025 Hrvoje Cavrak
 *
 * This program is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, version 3.
 *
 * See the file LICENSE for the full license text.
 */
#pragma once

#include <stdint.h>
#include "flash.h"
#include "packet.h"
#include "screen.h"
#include "vkeys.h"

typedef void (*action_handler_t)();

typedef struct { // Maps message type -> message handler function
    enum packet_type_e type;
    action_handler_t handler;
} uart_handler_t;

typedef struct {
    uint8_t modifier;                 // Which modifier is pressed
    uint8_t keys[KEYS_IN_USB_REPORT]; // Which keys need to be pressed
    uint8_t key_count;                // How many keys are pressed
    action_handler_t action_handler;  // What to execute when the key combination is detected
    bool pass_to_os;                  // True if we are to pass the key to the OS too
    bool acknowledge;                 // True if we are to notify the user about registering keypress
} hotkey_combo_t;

typedef struct TU_ATTR_PACKED {
    uint8_t buttons;
    int16_t x;
    int16_t y;
    int8_t wheel;
    int8_t pan;
    uint8_t mode;
} mouse_report_t;

typedef struct {
    uint8_t tip_pressure;
    uint8_t buttons; // Digitizer buttons
    uint16_t x;      // X coordinate (0-32767)
    uint16_t y;      // Y coordinate (0-32767)
} touch_report_t;

typedef struct {
    uint8_t instance;
    uint8_t report_id;
    uint8_t type;
    uint8_t len;
    uint8_t data[RAW_PACKET_LENGTH];
} hid_generic_pkt_t;

typedef enum { IDLE, READING_PACKET, PROCESSING_PACKET } receiver_state_t;

typedef struct {
    uint32_t address;         // Address we're sending to the other box
    uint32_t checksum;
    uint16_t version;
    bool byte_done;           // Has the byte been successfully transferred
    bool upgrade_in_progress; // True if firmware transfer from the other box is in progress
} fw_upgrade_state_t;

/*==============================================================================
 *  Key Remapping
 *  Each entry maps one source keycode (with optional modifier requirements) to
 *  a destination keycode/modifier, scoped to one or both outputs.
 *
 *  output_mask bits: bit 0 = OUTPUT_A, bit 1 = OUTPUT_B  (0x03 = both)
 *  dst_modifier:     0xFF = keep the original modifier unchanged
 *==============================================================================*/
#define MAX_KEY_REMAPS 32

typedef struct {
    uint8_t src_keycode;  // HID keycode to match; 0 = unused slot
    uint8_t dst_keycode;  // HID keycode to emit
    uint8_t src_modifier; // Modifier bits that must ALL be present (0 = any)
    uint8_t dst_modifier; // Modifier bits to emit (0xFF = preserve original)
    uint8_t output_mask;  // Which outputs this remap applies to
    uint8_t flags;        // Reserved for future use
} key_remap_t;

/*==============================================================================
 *  Caps Layer
 *
 *  While CapsLock is physically held the device enters a modal layer.
 *  Each entry intercepts one key and either:
 *    CAPS_ENTRY_CHORD   – emits a modifier+keycode chord to the active output
 *    CAPS_ENTRY_VIRTUAL – substitutes a virtual action keycode so the daemon
 *                         can intercept it and dispatch an OS-level action
 *    CAPS_ENTRY_JUMP_A  – immediately switch to output A, consume keypress
 *    CAPS_ENTRY_JUMP_B  – immediately switch to output B, consume keypress
 *    CAPS_ENTRY_SWAP    – toggle between outputs, consume keypress
 *
 *  Special case: Caps + Escape toggles the OS CapsLock state.
 *
 *  output_mask bits: bit 0 = OUTPUT_A, bit 1 = OUTPUT_B  (0x03 = both)
 *==============================================================================*/

#define CAPS_LAYER_MAX_ENTRIES 32

#define CAPS_ENTRY_CHORD   0  // chord: modifier + up to 4 keycodes
#define CAPS_ENTRY_VIRTUAL 1  // virtual: emit dualie_vkey(vaction_idx)
#define CAPS_ENTRY_JUMP_A  2  // switch to output A and consume keypress
#define CAPS_ENTRY_JUMP_B  3  // switch to output B and consume keypress
#define CAPS_ENTRY_SWAP    4  // toggle active output and consume keypress

typedef struct {
    uint8_t src_keycode;  // Physical key to intercept; 0 = unused slot
    uint8_t entry_type;   // CAPS_ENTRY_CHORD or CAPS_ENTRY_VIRTUAL
    uint8_t output_mask;  // Bit 0 = OUTPUT_A, bit 1 = OUTPUT_B
    uint8_t dst_modifier; // Modifier bits to emit (CHORD only)
    union {
        uint8_t dst_keycodes[4]; // Chord: up to 4 simultaneous keycodes (CHORD)
        struct {
            uint8_t vaction_idx; // Virtual action slot 0-31 (VIRTUAL)
            uint8_t _vpad[3];
        };
    };
} caps_layer_entry_t;     // 8 bytes – keep packed for flash layout

typedef struct {
    uint8_t             unmapped_passthrough; // 1 = pass, 0 = swallow unmapped keys
    uint8_t             _pad[3];              // align to 4 bytes
    caps_layer_entry_t  entries[CAPS_LAYER_MAX_ENTRIES];
} caps_layer_t;

typedef struct {
    uint32_t magic_header;
    uint32_t version;

    uint8_t force_mouse_boot_mode;
    uint8_t force_kbd_boot_protocol;

    uint8_t kbd_led_as_indicator;
    uint8_t hotkey_toggle;
    uint8_t enable_acceleration;

    uint8_t enforce_ports;
    uint16_t jump_threshold;

    output_t output[NUM_SCREENS];
    uint32_t _reserved;

    key_remap_t   key_remaps[MAX_KEY_REMAPS];        // Per-key remapping table
    caps_layer_t  caps_layer[NUM_SCREENS];            // Per-output caps modal layer

    // Keep checksum at the end of the struct
    uint32_t checksum;
} config_t;


/*==============================================================================
 *  Device State
 *==============================================================================*/
typedef struct {
    uint8_t kbd_dev_addr; // Address of the Keyboard device
    uint8_t kbd_instance; // Keyboard instance (d'uh - isn't this a useless comment)

    uint8_t keyboard_leds[NUM_SCREENS];  // State of keyboard LEDs (index 0 = A, index 1 = B)
    uint64_t last_activity[NUM_SCREENS]; // Timestamp of the last input activity (-||-)
    uint64_t core1_last_loop_pass;       // Timestamp of last core1 loop execution
    uint8_t active_output;               // Currently selected output (0 = A, 1 = B)
    uint8_t board_role;                  // Which board are we running on? (0 = A, 1 = B, etc.)

    hid_keyboard_report_t local_kbd_states[MAX_DEVICES]; // Store keyboard states
    hid_keyboard_report_t remote_kbd_state;              // Store combined remote keyboard state
    uint8_t max_kbd_idx;                                 // Store largest kbd_idx seen

    int16_t pointer_x; // Store and update the location of our mouse pointer
    int16_t pointer_y;
    int16_t mouse_buttons; // Store and update the state of mouse buttons

    config_t config;       // Device configuration, loaded from flash or defaults used
    queue_t hid_queue_out; // Queue that stores outgoing hid messages
    queue_t kbd_queue;     // Queue that stores keyboard reports
    queue_t mouse_queue;   // Queue that stores mouse reports
    queue_t uart_tx_queue; // Queue that stores outgoing packets

    hid_interface_t iface[MAX_DEVICES][MAX_INTERFACES]; // Store info about HID interfaces
    uart_packet_t in_packet;

    /* DMA */
    uint32_t dma_ptr;             // Stores info about DMA ring buffer last checked position
    uint32_t dma_rx_channel;      // DMA RX channel we're using to receive
    uint32_t dma_control_channel; // DMA channel that controls the RX transfer channel
    uint32_t dma_tx_channel;      // DMA TX channel we're using to send

    /* Firmware */
    fw_upgrade_state_t fw;           // State of the firmware upgrader
    firmware_metadata_t _running_fw; // RAM copy of running fw metadata
    bool reboot_requested;           // If set, stop updating watchdog
    uint64_t config_mode_timer;      // Counts how long are we to remain in config mode

    uint8_t page_buffer[FLASH_PAGE_SIZE]; // For firmware-over-serial upgrades

    /* WebHID config receive buffer – accumulates chunks from the browser.
     * Size matches CONFIG_T_SIZE (848 bytes) from daemon/src/serialize.rs.
     * Defined here to avoid a large VLA on the stack in tud_hid_set_report_cb. */
    uint8_t hid_config_rx[848];
    uint16_t hid_config_rx_bytes; // how many bytes have been written so far

    /* Connection status flags */
    bool tud_connected;      // True when TinyUSB device successfully connects
    bool keyboard_connected; // True when our keyboard is connected locally
    bool mouse_connected;    // True when our mouse is connected locally

    /* Feature flags */
    bool mouse_zoom;         // True when "mouse zoom" is enabled
    bool switch_lock;        // True when device is prevented from switching
    bool onboard_led_state;  // True when LED is ON
    bool relative_mouse;     // True when relative mouse mode is used
    bool gaming_mode;        // True when gaming mode is on (relative passthru + lock)
    bool config_mode_active; // True when config mode is active
    bool digitizer_active;   // True when digitizer Win/Mac workaround is active

    /* Caps layer runtime state */
    bool caps_held;          // True while CapsLock is physically held down
    bool caps_lock_on;       // Tracks logical OS CapsLock state (toggled by Caps+Esc)

    /* Onboard LED blinky (provide feedback when e.g. mouse connected) */
    int32_t blinks_left;     // How many blink transitions are left
    int32_t last_led_change; // Timestamp of the last time led state transitioned
} device_t;
/*==============================================================================*/


typedef struct {
    void (*exec)(device_t *state);
    uint64_t frequency;
    uint64_t next_run;
    bool *enabled;
} task_t;

enum os_type_e {
    LINUX   = 1,
    MACOS   = 2,
    WINDOWS = 3,
    ANDROID = 4,
    OTHER   = 255,
};

enum screen_pos_e {
    NONE   = 0,
    LEFT   = 1,
    RIGHT  = 2,
    MIDDLE = 3,
};

enum screensaver_mode_e {
    DISABLED   = 0,
    PONG       = 1,
    JITTER     = 2,
    MAX_SS_VAL = JITTER,
};

extern const config_t default_config;
extern const config_t ADDR_CONFIG[];
extern const uint8_t ADDR_FW_METADATA[];
extern const uint8_t ADDR_FW_RUNNING[];
extern const uint8_t ADDR_FW_STAGING[];
extern const uint8_t ADDR_DISK_IMAGE[];
