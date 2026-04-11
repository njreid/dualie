#pragma once
#include <stdint.h>
#include <stdbool.h>
#include <string.h>

#define KEYS_IN_USB_REPORT  6
#define HID_KEY_CAPS_LOCK   0x39
#define HID_KEY_ESCAPE      0x29

typedef struct {
    uint8_t modifier;
    uint8_t reserved;
    uint8_t keycode[KEYS_IN_USB_REPORT];
} hid_keyboard_report_t;
