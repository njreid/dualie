/// Thin fetch wrappers for the Dualie daemon REST API.

const BASE = '/api/v1';

async function request(method, path, body) {
  const opts = { method, headers: {} };
  if (body !== undefined) {
    opts.headers['Content-Type'] = 'application/json';
    opts.body = JSON.stringify(body);
  }
  const res = await fetch(BASE + path, opts);
  if (!res.ok) {
    const body = await res.json().catch(() => null);
    // A 5xx with no JSON body means the proxy couldn't reach the daemon
    const msg = body?.error
      ?? (res.status >= 500 ? 'Could not reach daemon — is it running?' : res.statusText);
    throw new Error(msg);
  }
  if (res.status === 204) return null;
  return res.json();
}

// ── Config ────────────────────────────────────────────────────────────────────

export const getConfig   = ()       => request('GET',  '/config');
export const putConfig   = (cfg)    => request('PUT',  '/config', cfg);

export async function downloadConfig() {
  const res = await fetch(BASE + '/config/download');
  if (!res.ok) throw new Error(await res.text());
  const blob = await res.blob();
  const url  = URL.createObjectURL(blob);
  const a    = document.createElement('a');
  a.href     = url;
  a.download = 'dualie-config.cbor';
  a.click();
  URL.revokeObjectURL(url);
}

export async function uploadConfig(file) {
  const buf = await file.arrayBuffer();
  const res = await fetch(BASE + '/config/upload', {
    method:  'POST',
    headers: { 'Content-Type': 'application/cbor' },
    body:    buf,
  });
  if (!res.ok) {
    const err = await res.json().catch(() => ({ error: res.statusText }));
    throw new Error(err.error ?? res.statusText);
  }
}

// ── Virtual actions ───────────────────────────────────────────────────────────

export const getActions  = (idx)              => request('GET', `/outputs/${idx}/actions`);
export const putAction   = (idx, slot, action) => request('PUT', `/outputs/${idx}/actions/${slot}`, action);

// ── Platform discovery ────────────────────────────────────────────────────────

export const getPlatformInfo = () => request('GET', '/platform/info');
export const getPlatformApps = () => request('GET', '/platform/apps');

// ── Keycodes table ────────────────────────────────────────────────────────────

let _keycodes = null;
export async function getKeycodes() {
  if (!_keycodes) {
    const res  = await fetch(BASE + '/keycodes');
    const raw  = await res.json();
    // keycodes.json stores keycodes as {"4":"A","5":"B",...}
    // Transform to [{hid: 4, label: "A"}, ...] sorted by HID code.
    const keys = Object.entries(raw.keycodes ?? {})
      .map(([hid, label]) => ({ hid: parseInt(hid, 10), label }))
      .sort((a, b) => a.hid - b.hid);
    _keycodes = {
      keys,
      modifiers:       raw.modifiers       ?? [],
      virtual_actions: raw.virtual_actions ?? [],
    };
  }
  return _keycodes;
}

// ── Device output tracking ────────────────────────────────────────────────────

export const setActiveOutput = (idx) => request('PUT', '/device/output', { index: idx });
