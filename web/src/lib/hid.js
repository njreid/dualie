/// WebHID layer for direct communication with the Dualie device.
///
/// The Pico presents as a composite HID device.  The browser opens the
/// vendor-specific interface (usage page 0xFF00) to read/write config structs
/// and firmware blobs without going through the daemon.

import { setActiveOutput } from './api.js';

// Dualie USB identifiers (must match firmware usb_descriptors.c)
const VENDOR_ID  = 0x1209;   // pid.codes open VID
const PRODUCT_ID = 0xC000;   // assigned in firmware usb_descriptors.c
const USAGE_PAGE = 0xFF00;   // vendor-specific (HID_USAGE_PAGE_VENDOR)
const USAGE      = 0x0001;   // matches TUD_HID_REPORT_DESC_DUALIE_CTRL usage

// Report IDs  (must match firmware hid_report_desc)
const REPORT_SWITCH_OUTPUT = 0x10;
const REPORT_CONFIG_CHUNK  = 0x11;
const REPORT_STATUS        = 0x13;

export class DualieHID {
  #device = null;

  get connected() { return this.#device != null; }

  /// Open the first matching Dualie device.
  async connect() {
    const devices = await navigator.hid.requestDevice({
      filters: [{ vendorId: VENDOR_ID, productId: PRODUCT_ID, usagePage: USAGE_PAGE, usage: USAGE }],
    });
    if (!devices.length) throw new Error('No Dualie device selected');
    this.#device = devices[0];
    await this.#device.open();

    // Listen for input reports (status / output-switch notifications)
    this.#device.addEventListener('inputreport', (e) => this.#onInputReport(e));
    return this;
  }

  async disconnect() {
    if (this.#device) {
      await this.#device.close();
      this.#device = null;
    }
  }

  // ── Output switching ──────────────────────────────────────────────────────

  /// Tell the device to switch to output 0 (A) or 1 (B).
  async switchOutput(idx) {
    await this.#send(REPORT_SWITCH_OUTPUT, new Uint8Array([idx & 1]));
  }

  // ── Config blob transfer ──────────────────────────────────────────────────

  /// Fetch the current daemon config as a firmware binary blob and push it
  /// to the device in 60-byte WebHID chunks.
  async pushConfig() {
    const res = await fetch('/api/v1/config/binary');
    if (!res.ok) throw new Error(`binary_config: ${res.statusText}`);
    const bytes = new Uint8Array(await res.arrayBuffer());
    await this.uploadConfigBlob(bytes);
  }

  /// Upload a raw config_t blob to the device in 60-byte chunks.
  async uploadConfigBlob(bytes) {
    const CHUNK = 60;
    for (let offset = 0; offset < bytes.length; offset += CHUNK) {
      const chunk     = bytes.slice(offset, offset + CHUNK);
      const padded    = new Uint8Array(CHUNK);
      padded.set(chunk);
      const offsetLE  = new Uint8Array(2);
      new DataView(offsetLE.buffer).setUint16(0, offset, /*little-endian*/true);
      const payload   = new Uint8Array(2 + CHUNK);
      payload.set(offsetLE);
      payload.set(padded, 2);
      await this.#send(REPORT_CONFIG_CHUNK, payload);
    }
  }

  // ── Private ───────────────────────────────────────────────────────────────

  async #send(reportId, data) {
    if (!this.#device) throw new Error('Not connected');
    await this.#device.sendReport(reportId, data);
  }

  #onInputReport(event) {
    const { reportId, data } = event;
    if (reportId === REPORT_STATUS) {
      const activeOutput = data.getUint8(0);
      // Keep daemon in sync so virtual action dispatch targets the right output
      setActiveOutput(activeOutput).catch(() => {});
      this.onOutputChange?.(activeOutput);
    }
  }
}

/// Singleton for the app to share
export const hid = new DualieHID();
