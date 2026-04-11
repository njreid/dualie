/*
 * Test runner: calls all test functions from test_caps_layer.c and
 * test_serialize.c, then prints "All tests passed." on success.
 * Any ASSERT failure calls exit(1) with a message to stderr.
 */

#include <stdio.h>

/* ── Declarations from test_caps_layer.c ────────────────────────────────── */
void test_caps_esc_consumed(void);
void test_chord_entry(void);
void test_unmapped_swallowed(void);
void test_unmapped_passthrough(void);

/* ── Declarations from test_serialize.c ─────────────────────────────────── */
void test_bad_magic_rejected(void);
void test_wrong_version_rejected(void);
void test_valid_blob_accepted(void);

int main(void) {
    /* caps layer */
    test_caps_esc_consumed();
    test_chord_entry();
    test_unmapped_swallowed();
    test_unmapped_passthrough();

    /* serialize / apply_hid_config */
    test_bad_magic_rejected();
    test_wrong_version_rejected();
    test_valid_blob_accepted();

    puts("All tests passed.");
    return 0;
}
