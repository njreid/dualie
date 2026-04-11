/// Tests for api.js (getKeycodes) and hid.js (DualieHID.uploadConfigBlob).

import { describe, it, expect, vi, beforeEach } from 'vitest';

// ── getKeycodes ───────────────────────────────────────────────────────────────

describe('getKeycodes', () => {
  beforeEach(() => {
    // Reset the module between tests so the _keycodes cache is cleared.
    vi.resetModules();
    vi.restoreAllMocks();
  });

  it('transforms keycodes object into sorted keys array', async () => {
    global.fetch = vi.fn().mockResolvedValue({
      ok: true,
      json: async () => ({
        keycodes: { '5': 'B', '4': 'A' },
        modifiers: [],
        virtual_actions: [],
      }),
    });

    // Fresh import after resetModules() so the cache is empty.
    const { getKeycodes } = await import('./api.js');
    const result = await getKeycodes();

    expect(result.keys).toEqual([
      { hid: 4, label: 'A' },
      { hid: 5, label: 'B' },
    ]);
  });
});

// ── DualieHID.uploadConfigBlob ────────────────────────────────────────────────

describe('DualieHID.uploadConfigBlob', () => {
  it('sends correct chunks for a 130-byte buffer', async () => {
    const { DualieHID } = await import('./hid.js');

    const hid = new DualieHID();

    // Build a 130-byte buffer: byte 0 = 0xAB, rest zeros.
    const bytes = new Uint8Array(130);
    bytes[0] = 0xAB;

    // Inject a fake connected device with a spy on sendReport.
    const sendReport = vi.fn().mockResolvedValue(undefined);
    // Access the private field via the instance; use Object.defineProperty trick.
    // DualieHID stores the device in a private field (#device).
    // We bypass this by monkey-patching the prototype's #send equivalent via
    // a subclass that overrides the private method is not possible in JS,
    // so instead we inject a fake device using the connect path.
    // Simpler: override sendReport on the device object directly by wiring up
    // a fake device before calling uploadConfigBlob.
    // We use the fact that #send calls this.#device.sendReport — so we set
    // the private field via a reflected approach.

    // Use a fake device injected through a test-only connect shim.
    const fakeDevice = {
      sendReport,
      open: vi.fn().mockResolvedValue(undefined),
      addEventListener: vi.fn(),
    };

    // Patch navigator.hid so connect() succeeds.
    global.navigator = {
      hid: {
        requestDevice: vi.fn().mockResolvedValue([fakeDevice]),
      },
    };

    await hid.connect();
    await hid.uploadConfigBlob(bytes);

    // 130 bytes / 60 bytes per chunk = 3 chunks (offsets 0, 60, 120)
    expect(sendReport).toHaveBeenCalledTimes(3);

    const calls = sendReport.mock.calls;

    // Each call is (reportId, payload) where payload is Uint8Array(62): 2 offset bytes + 60 data bytes.

    // Chunk 0: offset=0 (LE: 0x00 0x00), data[0]=0xAB
    const payload0 = calls[0][1];
    expect(payload0[0]).toBe(0);    // offset low byte
    expect(payload0[1]).toBe(0);    // offset high byte
    expect(payload0[2]).toBe(0xAB); // first data byte

    // Chunk 1: offset=60 (LE: 0x3C 0x00)
    const payload1 = calls[1][1];
    expect(payload1[0]).toBe(60);
    expect(payload1[1]).toBe(0);

    // Chunk 2: offset=120 (LE: 0x78 0x00)
    const payload2 = calls[2][1];
    expect(payload2[0]).toBe(120);
    expect(payload2[1]).toBe(0);
  });
});
