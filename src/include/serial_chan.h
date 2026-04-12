/*
 * serial_chan.h — CDC-ACM serial channel between RP2040 and the daemon.
 *
 * Replaces the F13–F24 virtual keycode hack.  When a caps-layer VIRTUAL
 * action fires the firmware calls `serial_send_virtual_action()` which sends
 *
 *   COBS({ "type": "virtual_action", "slot": <idx> })  0x00
 *
 * over the CDC-ACM interface.  The daemon receives it on /dev/ttyACM* and
 * dispatches the configured action entirely in software.
 *
 * Incoming frames from the daemon are received in `serial_task()` and
 * dispatched:
 *   - RebootToBootloader → calls reset_usb_boot(0,0) (USB MSC bootloader)
 *   - All other frames   → forwarded byte-for-byte over the inter-board UART
 *                          (the other board relays to its own daemon)
 *
 * The firmware also relays UART frames from the other board to the daemon
 * (see uart.c / serial_relay_from_uart()).
 */
#pragma once
#include <stdint.h>

/*
 * Send a VirtualAction { slot: idx } message over CDC-ACM.
 * Encodes as CBOR map, COBS-encodes, writes + 0x00 delimiter.
 * Non-blocking: drops silently if CDC is not connected.
 */
void serial_send_virtual_action(uint8_t idx);

/*
 * Send a FirmwareInfo { version: FIRMWARE_VERSION } message over CDC-ACM.
 * Called once when the daemon first opens the port (DTR raised).
 */
void serial_send_firmware_info(void);

/*
 * Poll for incoming CDC-ACM frames and dispatch them.
 * Signature matches task_t.exec so it can be registered in tasks_core0.
 */
void serial_chan_task(device_t *state);
