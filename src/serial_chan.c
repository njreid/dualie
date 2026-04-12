/*
 * serial_chan.c — CDC-ACM serial channel between RP2040 and the daemon.
 *
 * CBOR encoding notes
 * -------------------
 * All messages use the serde internal-tag format:
 *   { "type": "variant_name", <fields> }
 *
 * We hard-code the minimal CBOR for the two messages we need to send:
 *
 *   VirtualAction { slot: N }
 *     A2                            map(2)
 *     64 74 79 70 65                "type" (tstr 4)
 *     6E 76 69 72 74 75 61 6C      "virtual_action" (tstr 14)
 *        5F 61 63 74 69 6F 6E
 *     64 73 6C 6F 74                "slot" (tstr 4)
 *     NN                            uint(N)  for N < 24
 *     18 NN                         uint(N)  for N >= 24
 *
 *   FirmwareInfo { version: V }
 *     A2
 *     64 74 79 70 65
 *     6D 66 69 72 6D 77 61 72 65   "firmware_info" (tstr 13)
 *        5F 69 6E 66 6F
 *     67 76 65 72 73 69 6F 6E      "version" (tstr 7)
 *     1A VV VV VV VV               uint32(version)
 *
 * Received messages we care about:
 *   RebootToBootloader:
 *     A1 64 74 79 70 65 74 72 65 62 6F 6F 74 5F 74 6F
 *        5F 62 6F 6F 74 6C 6F 61 64 65 72
 *
 * All other received frames are relayed byte-for-byte over the inter-board UART.
 */
#include "include/serial_chan.h"
#include "include/cobs.h"
#include "include/main.h"
#include "tusb.h"
#include "pico/bootrom.h"

#include <string.h>
#include <stddef.h>

/* ── Firmware version ─────────────────────────────────────────────────────── */

#ifndef FIRMWARE_VERSION
#define FIRMWARE_VERSION 1u
#endif

/* ── Internal RX buffer ───────────────────────────────────────────────────── */

#define RX_BUF_SIZE 256

static uint8_t rx_buf[RX_BUF_SIZE];
static size_t  rx_len;

/* ── CBOR message templates ───────────────────────────────────────────────── */

/* Prefix for VirtualAction CBOR map (everything before the slot value). */
static const uint8_t VIRTUAL_ACTION_PREFIX[] = {
    0xA2,                   /* map(2) */
    0x64, 't','y','p','e',  /* "type" tstr(4) */
    0x6E,                   /* tstr(14) — "virtual_action" */
    'v','i','r','t','u','a','l','_','a','c','t','i','o','n',
    0x64, 's','l','o','t',  /* "slot" tstr(4) */
};

/* Prefix for FirmwareInfo CBOR map (everything before the version value). */
static const uint8_t FIRMWARE_INFO_PREFIX[] = {
    0xA2,                         /* map(2) */
    0x64, 't','y','p','e',        /* "type" tstr(4) */
    0x6D,                         /* tstr(13) — "firmware_info" */
    'f','i','r','m','w','a','r','e','_','i','n','f','o',
    0x67, 'v','e','r','s','i','o','n',  /* "version" tstr(7) */
};

/* CBOR bytes for RebootToBootloader (entire message, no fields). */
static const uint8_t REBOOT_TO_BOOTLOADER_CBOR[] = {
    0xA1,                   /* map(1) */
    0x64, 't','y','p','e',  /* "type" tstr(4) */
    0x74,                   /* tstr(20) — "reboot_to_bootloader" */
    'r','e','b','o','o','t','_','t','o','_','b','o','o','t','l','o','a','d','e','r',
};

/* ── Send helpers ─────────────────────────────────────────────────────────── */

/* Write `cbor[0..len]` as a COBS frame + 0x00 delimiter to CDC. */
static void send_frame(const uint8_t *cbor, size_t len) {
    if (!tud_cdc_connected())
        return;

    uint8_t encoded[COBS_ENCODED_MAX(256)];
    size_t enc_len = cobs_encode(cbor, len, encoded);
    encoded[enc_len++] = 0x00; /* frame delimiter */
    tud_cdc_write(encoded, enc_len);
    tud_cdc_write_flush();
}

/* ── Public API ───────────────────────────────────────────────────────────── */

void serial_send_virtual_action(uint8_t idx) {
    /* Build:  prefix + 1–2 byte CBOR uint for the slot. */
    uint8_t cbor[sizeof(VIRTUAL_ACTION_PREFIX) + 2];
    size_t  cbor_len = sizeof(VIRTUAL_ACTION_PREFIX);

    memcpy(cbor, VIRTUAL_ACTION_PREFIX, sizeof(VIRTUAL_ACTION_PREFIX));

    if (idx < 24) {
        cbor[cbor_len++] = idx;           /* CBOR: direct uint */
    } else {
        cbor[cbor_len++] = 0x18;          /* CBOR: uint8 follows */
        cbor[cbor_len++] = idx;
    }

    send_frame(cbor, cbor_len);
}

void serial_send_firmware_info(void) {
    /* Build:  prefix + 5-byte CBOR uint32 for the version. */
    uint8_t cbor[sizeof(FIRMWARE_INFO_PREFIX) + 5];
    size_t  cbor_len = sizeof(FIRMWARE_INFO_PREFIX);

    memcpy(cbor, FIRMWARE_INFO_PREFIX, sizeof(FIRMWARE_INFO_PREFIX));
    cbor[cbor_len++] = 0x1A;                        /* CBOR: uint32 follows */
    cbor[cbor_len++] = (FIRMWARE_VERSION >> 24) & 0xFF;
    cbor[cbor_len++] = (FIRMWARE_VERSION >> 16) & 0xFF;
    cbor[cbor_len++] = (FIRMWARE_VERSION >>  8) & 0xFF;
    cbor[cbor_len++] =  FIRMWARE_VERSION        & 0xFF;

    send_frame(cbor, cbor_len);
}

/* ── RX dispatch ──────────────────────────────────────────────────────────── */

static bool is_reboot_to_bootloader(const uint8_t *decoded, size_t len) {
    if (len != sizeof(REBOOT_TO_BOOTLOADER_CBOR))
        return false;
    return memcmp(decoded, REBOOT_TO_BOOTLOADER_CBOR,
                  sizeof(REBOOT_TO_BOOTLOADER_CBOR)) == 0;
}

static void dispatch_frame(const uint8_t *raw, size_t raw_len) {
    uint8_t decoded[RX_BUF_SIZE];
    size_t  dec_len = cobs_decode(raw, raw_len, decoded);

    if (dec_len == 0)
        return; /* corrupt frame */

    if (is_reboot_to_bootloader(decoded, dec_len)) {
        /* Put the RP2040 into USB MSC bootloader mode immediately. */
        reset_usb_boot(0, 0);
        /* Not reached — the board resets. */
    }

    /* TODO Phase 6: relay other frames to the other board over UART.
     * The DeskHop UART uses a fixed packet_t structure; a new packet type
     * for carrying arbitrary COBS frames needs to be added before relay
     * can be implemented. */
    (void)raw;
    (void)raw_len;
}

/* ── Main task ────────────────────────────────────────────────────────────── */

void serial_chan_task(device_t *state) {
    (void)state;
    /* Detect when the daemon opens the port (DTR raised) and send firmware info. */
    static bool last_dtr = false;
    bool        dtr      = tud_cdc_connected();

    if (dtr && !last_dtr)
        serial_send_firmware_info();
    last_dtr = dtr;

    /* Read available bytes; accumulate until 0x00 frame delimiter. */
    while (tud_cdc_available()) {
        uint8_t b;
        if (tud_cdc_read(&b, 1) == 0)
            break;

        if (b == 0x00) {
            /* End of frame — dispatch and reset. */
            if (rx_len > 0) {
                dispatch_frame(rx_buf, rx_len);
                rx_len = 0;
            }
        } else {
            if (rx_len < RX_BUF_SIZE)
                rx_buf[rx_len++] = b;
            else
                rx_len = 0; /* overflow — discard and resync */
        }
    }
}
