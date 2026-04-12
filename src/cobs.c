/*
 * cobs.c — COBS encoder/decoder implementation.
 *
 * Reference: Stuart Cheshire & Mary Baker,
 *   "Consistent Overhead Byte Stuffing", IEEE/ACM Trans. Networking, 1999.
 */
#include "include/cobs.h"
#include <string.h>

size_t cobs_encode(const uint8_t *src, size_t src_len, uint8_t *dst) {
    size_t read_idx  = 0;
    size_t write_idx = 1;          /* skip the first code byte placeholder */
    size_t code_idx  = 0;          /* where the current code byte lives */
    uint8_t code     = 1;          /* distance to the next 0x00 */

    while (read_idx < src_len) {
        if (src[read_idx] == 0x00) {
            /* Terminate the current run. */
            dst[code_idx] = code;
            code_idx  = write_idx++;
            code      = 1;
        } else {
            dst[write_idx++] = src[read_idx];
            code++;
            if (code == 0xFF) {
                /* Run of 254 non-zero bytes — emit code and start a new block. */
                dst[code_idx] = code;
                code_idx  = write_idx++;
                code      = 1;
            }
        }
        read_idx++;
    }

    dst[code_idx] = code;
    return write_idx;
}

size_t cobs_decode(const uint8_t *src, size_t src_len, uint8_t *dst) {
    if (src_len == 0)
        return 0;

    size_t read_idx  = 0;
    size_t write_idx = 0;

    while (read_idx < src_len) {
        uint8_t code = src[read_idx++];
        if (code == 0x00)
            return 0;  /* 0x00 is not valid inside a COBS frame */

        /* Copy (code-1) data bytes. */
        for (uint8_t i = 1; i < code; i++) {
            if (read_idx >= src_len)
                return 0;  /* truncated frame */
            dst[write_idx++] = src[read_idx++];
        }

        /* Emit an implicit 0x00 unless this is the final code block. */
        if (code < 0xFF && read_idx < src_len)
            dst[write_idx++] = 0x00;
    }

    return write_idx;
}
