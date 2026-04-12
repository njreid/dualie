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

#include "usb_descriptors.h"
#include "main.h"
#include "tusb.h"

//--------------------------------------------------------------------+
// Device Descriptors
//--------------------------------------------------------------------+

                                        // https://github.com/raspberrypi/usb-pid
tusb_desc_device_t const desc_device_config = DEVICE_DESCRIPTOR(0x2e8a, 0x107c);

                                        // https://pid.codes/1209/C000/
tusb_desc_device_t const desc_device = DEVICE_DESCRIPTOR(0x1209, 0xc000);

// Invoked when received GET DEVICE DESCRIPTOR
// Application return pointer to descriptor
uint8_t const *tud_descriptor_device_cb(void) {
    if (global_state.config_mode_active)
        return (uint8_t const *)&desc_device_config;
    else
        return (uint8_t const *)&desc_device;
}

//--------------------------------------------------------------------+
// HID Report Descriptor
//--------------------------------------------------------------------+

// Relative mouse is used to overcome limitations of multiple desktops on MacOS and Windows

uint8_t const desc_hid_report[] = {TUD_HID_REPORT_DESC_KEYBOARD(HID_REPORT_ID(REPORT_ID_KEYBOARD)),
                                   TUD_HID_REPORT_DESC_ABS_MOUSE(HID_REPORT_ID(REPORT_ID_MOUSE)),
                                   TUD_HID_REPORT_DESC_CONSUMER_CTRL(HID_REPORT_ID(REPORT_ID_CONSUMER)),
                                   TUD_HID_REPORT_DESC_SYSTEM_CONTROL(HID_REPORT_ID(REPORT_ID_SYSTEM))
                                   };

uint8_t const desc_hid_report_relmouse[] = {TUD_HID_REPORT_DESC_MOUSEHELP(HID_REPORT_ID(REPORT_ID_RELMOUSE))};

// Dualie WebHID control interface replaces the legacy vendor descriptor.
// Must match hid.js REPORT_SWITCH_OUTPUT / REPORT_CONFIG_CHUNK / REPORT_STATUS.
uint8_t const desc_hid_report_vendor[] = {TUD_HID_REPORT_DESC_DUALIE_CTRL()};


// Invoked when received GET HID REPORT DESCRIPTOR
// Application return pointer to descriptor
// Descriptor contents must exist long enough for transfer to complete
uint8_t const *tud_hid_descriptor_report_cb(uint8_t instance) {
    if (global_state.config_mode_active)
        if (instance == ITF_NUM_HID_VENDOR)
            return desc_hid_report_vendor;

    switch(instance) {
        case ITF_NUM_HID:
            return desc_hid_report;
        case ITF_NUM_HID_REL_M:
            return desc_hid_report_relmouse;
        default:
            return desc_hid_report;
    }
}

bool tud_mouse_report(uint8_t mode, uint8_t buttons, int16_t x, int16_t y, int8_t wheel, int8_t pan) {
    mouse_report_t report = {.buttons = buttons, .wheel = wheel, .x = x, .y = y, .mode = mode, .pan = pan};
    uint8_t instance = ITF_NUM_HID;
    uint8_t report_id = REPORT_ID_MOUSE;

    if (mode == RELATIVE) {
        instance = ITF_NUM_HID_REL_M;
        report_id = REPORT_ID_RELMOUSE;
    }

    return tud_hid_n_report(instance, report_id, &report, sizeof(report));
}


//--------------------------------------------------------------------+
// String Descriptors
//--------------------------------------------------------------------+

// array of pointer to string descriptors
char const *string_desc_arr[] = {
    (const char[]){0x09, 0x04}, // 0: supported language English (0x0409)
    "Hrvoje Cavrak",            // 1: Manufacturer
    "Dualie Switch",            // 2: Product
    "0",                        // 3: Serial (replaced with chip ID at runtime)
    "Dualie Mouse Helper",      // 4: Relative-mouse HID interface
    "Dualie Config",            // 5: Vendor/WebHID config interface
    "Dualie Disk",              // 6: MSC disk interface
    "Dualie Serial",            // 7: CDC-ACM daemon control channel
};

// String Descriptor Index
enum {
    STRID_LANGID = 0,
    STRID_MANUFACTURER,
    STRID_PRODUCT,
    STRID_SERIAL,
    STRID_MOUSE,
    STRID_VENDOR,
    STRID_DISK,
    STRID_CDC,
};

static uint16_t _desc_str[32];

// Invoked when received GET STRING DESCRIPTOR request
// Application return pointer to descriptor, whose contents must exist long enough for transfer to
// complete
uint16_t const *tud_descriptor_string_cb(uint8_t index, uint16_t langid) {
    (void)langid;

    uint8_t chr_count;

    // 2 (hex) characters for every byte + 1 '\0' for string end
    static char serial_number[PICO_UNIQUE_BOARD_ID_SIZE_BYTES * 2 + 1] = {0};

    if (!serial_number[0]) {
       pico_get_unique_board_id_string(serial_number, sizeof(serial_number));
    }

    if (index == 0) {
        memcpy(&_desc_str[1], string_desc_arr[0], 2);
        chr_count = 1;
    } else {
        // Note: the 0xEE index string is a Microsoft OS 1.0 Descriptors.
        // https://docs.microsoft.com/en-us/windows-hardware/drivers/usbcon/microsoft-defined-usb-descriptors

        if (!(index < sizeof(string_desc_arr) / sizeof(string_desc_arr[0])))
            return NULL;

        const char *str = (index == STRID_SERIAL) ? serial_number : string_desc_arr[index];

        // Cap at max char
        chr_count = strlen(str);
        if (chr_count > 31)
            chr_count = 31;

        // Convert ASCII string into UTF-16
        for (uint8_t i = 0; i < chr_count; i++) {
            _desc_str[1 + i] = str[i];
        }
    }

    // first byte is length (including header), second byte is string type
    _desc_str[0] = (TUSB_DESC_STRING << 8) | (2 * chr_count + 2);

    return _desc_str;
}

//--------------------------------------------------------------------+
// Configuration Descriptor
//--------------------------------------------------------------------+

#define EPNUM_HID        0x81
#define EPNUM_HID_REL_M  0x82
#define EPNUM_HID_VENDOR 0x83

#define EPNUM_MSC_OUT    0x04
#define EPNUM_MSC_IN     0x84

/* CDC-ACM (daemon control channel) — always present in both configurations. */
#define EPNUM_CDC_NOTIF  0x85
#define EPNUM_CDC_OUT    0x06
#define EPNUM_CDC_IN     0x86

/*
 * CDC-ACM takes 2 interface slots (comm + data), so:
 *   Normal mode:  HID(0) + HID_REL_M(1) + CDC(2+3)           → 4 interfaces
 *   Config mode:  HID(0) + HID_REL_M(1) + CDC(2+3) +
 *                 HID_VENDOR(4) + MSC(5)                      → 6 interfaces
 */
#define ITF_NUM_TOTAL        4
#define ITF_NUM_TOTAL_CONFIG 6

#define CONFIG_TOTAL_LEN \
    (TUD_CONFIG_DESC_LEN + 2 * TUD_HID_DESC_LEN + TUD_CDC_DESC_LEN)

#define CONFIG_TOTAL_LEN_CFG \
    (TUD_CONFIG_DESC_LEN + 3 * TUD_HID_DESC_LEN + TUD_MSC_DESC_LEN + TUD_CDC_DESC_LEN)


uint8_t const desc_configuration[] = {
    // Config number, interface count, string index, total length, attribute, power in mA
    TUD_CONFIG_DESCRIPTOR(1, ITF_NUM_TOTAL, 0, CONFIG_TOTAL_LEN, TUSB_DESC_CONFIG_ATT_REMOTE_WAKEUP, 500),

    // Interface 0: HID (keyboard + mouse + consumer)
    TUD_HID_DESCRIPTOR(ITF_NUM_HID,
                       STRID_PRODUCT,
                       HID_ITF_PROTOCOL_NONE,
                       sizeof(desc_hid_report),
                       EPNUM_HID,
                       CFG_TUD_HID_EP_BUFSIZE,
                       1),

    // Interface 1: HID (relative mouse helper)
    TUD_HID_DESCRIPTOR(ITF_NUM_HID_REL_M,
                       STRID_MOUSE,
                       HID_ITF_PROTOCOL_NONE,
                       sizeof(desc_hid_report_relmouse),
                       EPNUM_HID_REL_M,
                       CFG_TUD_HID_EP_BUFSIZE,
                       1),

    // Interfaces 2+3: CDC-ACM daemon control channel
    TUD_CDC_DESCRIPTOR(ITF_NUM_CDC, STRID_CDC,
                       EPNUM_CDC_NOTIF, 8,
                       EPNUM_CDC_OUT, EPNUM_CDC_IN, CFG_TUD_CDC_EP_BUFSIZE),
};

uint8_t const desc_configuration_config[] = {
    // Config number, interface count, string index, total length, attribute, power in mA
    TUD_CONFIG_DESCRIPTOR(1, ITF_NUM_TOTAL_CONFIG, 0, CONFIG_TOTAL_LEN_CFG, TUSB_DESC_CONFIG_ATT_REMOTE_WAKEUP, 500),

    // Interface 0: HID (keyboard + mouse + consumer)
    TUD_HID_DESCRIPTOR(ITF_NUM_HID,
                       STRID_PRODUCT,
                       HID_ITF_PROTOCOL_NONE,
                       sizeof(desc_hid_report),
                       EPNUM_HID,
                       CFG_TUD_HID_EP_BUFSIZE,
                       1),

    // Interface 1: HID (relative mouse helper)
    TUD_HID_DESCRIPTOR(ITF_NUM_HID_REL_M,
                       STRID_MOUSE,
                       HID_ITF_PROTOCOL_NONE,
                       sizeof(desc_hid_report_relmouse),
                       EPNUM_HID_REL_M,
                       CFG_TUD_HID_EP_BUFSIZE,
                       1),

    // Interfaces 2+3: CDC-ACM daemon control channel
    TUD_CDC_DESCRIPTOR(ITF_NUM_CDC, STRID_CDC,
                       EPNUM_CDC_NOTIF, 8,
                       EPNUM_CDC_OUT, EPNUM_CDC_IN, CFG_TUD_CDC_EP_BUFSIZE),

    // Interface 4: HID vendor/WebHID config
    TUD_HID_DESCRIPTOR(ITF_NUM_HID_VENDOR,
                       STRID_VENDOR,
                       HID_ITF_PROTOCOL_NONE,
                       sizeof(desc_hid_report_vendor),
                       EPNUM_HID_VENDOR,
                       CFG_TUD_HID_EP_BUFSIZE,
                       1),

    // Interface 5: MSC disk
    TUD_MSC_DESCRIPTOR(ITF_NUM_MSC,
                       STRID_DISK,
                       EPNUM_MSC_OUT,
                       EPNUM_MSC_IN,
                       64),
};

uint8_t const *tud_descriptor_configuration_cb(uint8_t index) {
    (void)index; // for multiple configurations

    if (global_state.config_mode_active)
        return desc_configuration_config;
    else
        return desc_configuration;
}
