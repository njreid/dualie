<script>
  import { onMount } from 'svelte';
  import { getConfig, putConfig, getKeycodes } from '../lib/api.js';

  let { activeOutput } = $props();

  let config   = $state(null);
  let keycodes = $state({ keys: [] });
  let error    = $state('');
  let saving   = $state(false);

  // HID modifier byte bit definitions (must match firmware / config.rs)
  const MODIFIERS = [
    { label: 'LCtrl',  bit: 0x01 },
    { label: 'LShift', bit: 0x02 },
    { label: 'LAlt',   bit: 0x04 },
    { label: 'LMeta',  bit: 0x08 },
    { label: 'RCtrl',  bit: 0x10 },
    { label: 'RShift', bit: 0x20 },
    { label: 'RAlt',   bit: 0x40 },
    { label: 'RMeta',  bit: 0x80 },
  ];

  function modLabel(byte) {
    const names = MODIFIERS.filter(m => byte & m.bit).map(m => m.label);
    return names.length ? names.join('+') : '—';
  }

  onMount(async () => {
    try {
      [config, keycodes] = await Promise.all([getConfig(), getKeycodes()]);
    } catch (e) {
      error = e.message;
    }
  });

  async function save() {
    saving = true;
    error  = '';
    try {
      await putConfig(config);
    } catch (e) {
      error = e.message;
    } finally {
      saving = false;
    }
  }

  function addRemap() {
    config.outputs[activeOutput].key_remaps.push({
      src_keycode: 0, dst_keycode: 0,
      src_modifier: 0, dst_modifier: 0,
      output_mask: 3, flags: 0,
    });
  }
  function removeRemap(i) {
    config.outputs[activeOutput].key_remaps.splice(i, 1);
  }

  function addModRemap() {
    if (!config.outputs[activeOutput].modifier_remaps)
      config.outputs[activeOutput].modifier_remaps = [];
    config.outputs[activeOutput].modifier_remaps.push({ src: 0x01, dst: 0x04 });
  }
  function removeModRemap(i) {
    config.outputs[activeOutput].modifier_remaps.splice(i, 1);
  }

  function toggleModBit(entry, field, bit) {
    entry[field] ^= bit;
  }
</script>

{#if config}
  {@const modRemaps = config.outputs[activeOutput].modifier_remaps ?? []}
  {@const remaps    = config.outputs[activeOutput].key_remaps ?? []}
  <section>
    <h2>Key Remaps — Output {activeOutput === 0 ? 'A' : 'B'}</h2>

    <!-- ── Modifier remaps ──────────────────────────────────────────────── -->
    <article>
      <header>Modifier remaps</header>
      <p>Transform modifier keys on every keystroke — e.g. swap Ctrl and Alt
         between machines.</p>

      {#each modRemaps as entry, i}
        <div class="mod-remap-row">
          <div class="mod-picker">
            {#each MODIFIERS as m}
              <label class="mod-chip" class:active={entry.src & m.bit}>
                <input type="checkbox" checked={!!(entry.src & m.bit)}
                       onchange={() => toggleModBit(entry, 'src', m.bit)} />
                {m.label}
              </label>
            {/each}
          </div>

          <span class="arrow">→</span>

          <div class="mod-picker">
            {#each MODIFIERS as m}
              <label class="mod-chip" class:active={entry.dst & m.bit}>
                <input type="checkbox" checked={!!(entry.dst & m.bit)}
                       onchange={() => toggleModBit(entry, 'dst', m.bit)} />
                {m.label}
              </label>
            {/each}
          </div>

          <button class="outline secondary icon-btn"
                  onclick={() => removeModRemap(i)}>✕</button>
        </div>
      {/each}

      <button class="outline" onclick={addModRemap}>+ Add modifier remap</button>
    </article>

    <!-- ── Key remaps ────────────────────────────────────────────────────── -->
    <article>
      <header>Key remaps</header>
      <p>Remap individual keys before they are sent to the active output.</p>

    {#if error}<p class="error">{error}</p>{/if}

    <div class="table-container">
      <table role="grid">
        <thead>
          <tr>
            <th>Source key</th>
            <th>→ Dest key</th>
            <th></th>
          </tr>
        </thead>
        <tbody>
          {#each remaps as remap, i}
            <tr>
              <td>
                <select bind:value={remap.src_keycode}>
                  <option value={0}>— key —</option>
                  {#each keycodes.keys ?? [] as k}
                    <option value={k.hid}>{k.label}</option>
                  {/each}
                </select>
              </td>
              <td>
                <select bind:value={remap.dst_keycode}>
                  <option value={0}>— key —</option>
                  {#each keycodes.keys ?? [] as k}
                    <option value={k.hid}>{k.label}</option>
                  {/each}
                </select>
              </td>
              <td>
                <button class="outline secondary" onclick={() => removeRemap(i)}>✕</button>
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>

      <div class="actions">
        <button class="outline" onclick={addRemap}>+ Add remap</button>
      </div>
    </article>

    <div class="actions">
      <button onclick={save} aria-busy={saving} disabled={saving}>Save all</button>
    </div>
  </section>
{:else if error}
  <p class="error">{error}</p>
{:else}
  <p aria-busy="true">Loading…</p>
{/if}

<style>
  .error   { color: var(--pico-color-red-500); }
  .actions { display: flex; gap: 0.5rem; margin-top: 1rem; }

  .mod-remap-row {
    display: flex; align-items: center; gap: 0.75rem;
    flex-wrap: wrap; margin-bottom: 0.75rem;
  }
  .mod-picker { display: flex; flex-wrap: wrap; gap: 0.3rem; }
  .mod-chip {
    display: inline-flex; align-items: center; gap: 0.2rem;
    padding: 0.15rem 0.4rem; border-radius: 4px; font-size: 0.8rem;
    border: 1px solid var(--pico-muted-border-color); cursor: pointer;
    user-select: none;
  }
  .mod-chip.active {
    background: var(--pico-primary-background);
    border-color: var(--pico-primary-border);
    color: var(--pico-primary-inverse);
  }
  .mod-chip input { display: none; }
  .arrow { font-size: 1.2rem; flex-shrink: 0; }
  .icon-btn { padding: 0.2rem 0.5rem; }
</style>
