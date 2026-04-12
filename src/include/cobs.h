/*
 * cobs.h — Consistent Overhead Byte Stuffing encoder/decoder.
 *
 * COBS guarantees the encoded output contains no 0x00 bytes.
 * A 0x00 byte is appended by the caller as a frame delimiter.
 *
 * Worst-case encoded length: input_len + 1 + ceil(input_len / 254).
 */
#pragma once
#include <stdint.h>
#include <stddef.h>

/*
 * Encode `src[0..src_len]` into `dst`.
 * `dst` must be at least COBS_ENCODED_MAX(src_len) bytes.
 * Returns the number of encoded bytes written to `dst` (NOT including
 * any trailing 0x00 frame delimiter — the caller appends that).
 */
size_t cobs_encode(const uint8_t *src, size_t src_len, uint8_t *dst);

/*
 * Decode `src[0..src_len]` (without the trailing 0x00 delimiter) into `dst`.
 * `dst` must be at least `src_len` bytes.
 * Returns the number of decoded bytes on success, or 0 on error.
 */
size_t cobs_decode(const uint8_t *src, size_t src_len, uint8_t *dst);

/* Maximum encoded output size for a given input length. */
#define COBS_ENCODED_MAX(n) ((n) + 1 + ((n) / 254))
