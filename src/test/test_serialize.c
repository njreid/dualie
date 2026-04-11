/*
 * Self-contained test for the apply_hid_config validation logic.
 *
 * The validation is inlined here (no pico-sdk or TinyUSB needed).
 * We reproduce the exact checks from src/usb.c: magic, version, checksum.
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>
#include <stdbool.h>

#define ASSERT(cond) \
    do { \
        if (!(cond)) { \
            fprintf(stderr, "FAIL %s:%d: %s\n", __FILE__, __LINE__, #cond); \
            exit(1); \
        } \
    } while (0)

/* ── Firmware constants (from config.h / usb.c) ─────────────────────────── */

#define FIRMWARE_MAGIC          0xB00B1E5u
#define CURRENT_CONFIG_VERSION  10u
#define CONFIG_T_SIZE           848u

/* ── Inline CRC-32 (mirrors daemon/src/serialize.rs and firmware utils.c) ── */

static uint32_t calc_crc32(const uint8_t *data, size_t len) {
    uint32_t crc = 0xFFFFFFFFu;
    for (size_t n = 0; n < len; n++) {
        crc ^= (uint32_t)data[n] << 24;
        for (int b = 0; b < 8; b++) {
            crc = (crc & 0x80000000u) ? (crc << 1) ^ 0x04C11DB7u : (crc << 1);
        }
    }
    return ~crc;
}

/* ── Minimal config_t header for validation ─────────────────────────────── */

typedef struct {
    uint32_t magic_header;
    uint32_t version;
    uint8_t  _rest[CONFIG_T_SIZE - 12]; /* flags … caps_layer */
    uint32_t checksum;
} config_header_t;

/* Compile-time size check */
_Static_assert(sizeof(config_header_t) == CONFIG_T_SIZE,
               "config_header_t size mismatch");

/* ── Mock save_config ────────────────────────────────────────────────────── */

static int save_config_calls = 0;

static void mock_save_config(void) {
    save_config_calls++;
}

/* ── Inline apply_hid_config logic (mirrors src/usb.c) ──────────────────── */

static void apply_hid_config_test(uint8_t *rx_buf) {
    config_header_t *rx = (config_header_t *)rx_buf;

    if (rx->magic_header != FIRMWARE_MAGIC)      return;
    if (rx->version      != CURRENT_CONFIG_VERSION) return;

    uint8_t expected = (uint8_t)calc_crc32(rx_buf, CONFIG_T_SIZE - sizeof(uint32_t));
    if ((uint8_t)rx->checksum != expected)       return;

    mock_save_config();
}

/* ── Helper: build a valid blob ─────────────────────────────────────────── */

static void build_valid_blob(uint8_t *buf) {
    memset(buf, 0, CONFIG_T_SIZE);
    config_header_t *h = (config_header_t *)buf;
    h->magic_header = FIRMWARE_MAGIC;
    h->version      = CURRENT_CONFIG_VERSION;
    /* Compute and store checksum */
    uint32_t crc = calc_crc32(buf, CONFIG_T_SIZE - sizeof(uint32_t));
    h->checksum = (uint8_t)crc; /* truncated to uint8_t, stored in uint32 */
}

/* ── Test cases ──────────────────────────────────────────────────────────── */

void test_bad_magic_rejected(void) {
    uint8_t buf[CONFIG_T_SIZE];
    build_valid_blob(buf);

    /* Corrupt the magic */
    config_header_t *h = (config_header_t *)buf;
    h->magic_header = 0xDEADBEEFu;

    save_config_calls = 0;
    apply_hid_config_test(buf);
    ASSERT(save_config_calls == 0);
}

void test_wrong_version_rejected(void) {
    uint8_t buf[CONFIG_T_SIZE];
    build_valid_blob(buf);

    config_header_t *h = (config_header_t *)buf;
    h->version = CURRENT_CONFIG_VERSION + 1;

    save_config_calls = 0;
    apply_hid_config_test(buf);
    ASSERT(save_config_calls == 0);
}

void test_valid_blob_accepted(void) {
    uint8_t buf[CONFIG_T_SIZE];
    build_valid_blob(buf);

    save_config_calls = 0;
    apply_hid_config_test(buf);
    ASSERT(save_config_calls == 1);
}
