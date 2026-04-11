<script>
  import { onMount } from 'svelte';
  import { getConfig, putConfig, getKeycodes } from '../lib/api.js';

  let { activeOutput } = $props();

  // Local copy of the relevant config slice
  let config       = $state(null);
  let keycodes     = $state({ keys: [], modifiers: [] });
  let error        = $state('');
  let saving       = $state(false);

  onMount(async () => {
    try {
      [config, keycodes] = await Promise.all([getConfig(), getKeycodes()]);
    } catch (e) {
      error = e.message;
    }
  });

  // Helper: readable name for a HID keycode
  function keyName(code) {
    if (!code) return '—';
    const entry = keycodes.keys?.find((k) => k.hid === code);
    return entry ? entry.label : `0x${code.toString(16).padStart(2, '0')}`;
  }

  const MOD_NAMES = ['LCtrl','LShift','LAlt','LMeta','RCtrl','RShift','RAlt','RMeta'];
  function modString(byte) {
    return MOD_NAMES.filter((_, i) => byte & (1 << i)).join('+') || '—';
  }

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

  function addEntry() {
    const layer = config.outputs[activeOutput].caps_layer;
    layer.entries.push({
      src_keycode: 0,
      entry_type: 0,
      output_mask: 3,
      dst_modifier: 0,
      dst_keycodes: [0, 0, 0, 0],
    });
  }

  function removeEntry(i) {
    config.outputs[activeOutput].caps_layer.entries.splice(i, 1);
  }
</script>

{#if config}
  {@const layer = config.outputs[activeOutput].caps_layer}
  <section>
    <h2>Caps Layer — Output {activeOutput === 0 ? 'A' : 'B'}</h2>
    <p>While CapsLock is held, these key mappings take effect. Caps+Esc toggles OS CapsLock.</p>

    <label>
      <input type="checkbox" bind:checked={layer.unmapped_passthrough} />
      Pass unmapped keys through (otherwise swallow them)
    </label>

    {#if error}<p class="error">{error}</p>{/if}

    <div class="table-container">
      <table role="grid">
        <thead>
          <tr>
            <th>Src Key</th>
            <th>Type</th>
            <th>Dst Mods</th>
            <th>Dst Keys / VSlot</th>
            <th></th>
          </tr>
        </thead>
        <tbody>
          {#each layer.entries as entry, i}
            <tr>
              <td>
                <select bind:value={entry.src_keycode}>
                  <option value={0}>— key —</option>
                  {#each keycodes.keys ?? [] as k}
                    <option value={k.hid}>{k.label}</option>
                  {/each}
                </select>
              </td>
              <td>
                <select bind:value={entry.entry_type}>
                  <option value={0}>Chord</option>
                  <option value={1}>Virtual action</option>
                  <option value={2}>Jump → A</option>
                  <option value={3}>Jump → B</option>
                  <option value={4}>Swap</option>
                </select>
              </td>
              <td>
                <div class="mod-grid">
                  {#each MOD_NAMES as name, bit}
                    <label class="mod-label">
                      <input type="checkbox"
                             checked={!!(entry.dst_modifier & (1 << bit))}
                             onchange={(e) => {
                               if (e.target.checked) entry.dst_modifier |= (1 << bit);
                               else entry.dst_modifier &= ~(1 << bit);
                             }} />
                      {name}
                    </label>
                  {/each}
                </div>
              </td>
              <td>
                {#if entry.entry_type === 0}
                  <!-- Chord: modifier bits + up to 4 dst keycodes -->
                  {#each [0,1,2,3] as ki}
                    <select bind:value={entry.dst_keycodes[ki]} style="margin:2px">
                      <option value={0}>—</option>
                      {#each keycodes.keys ?? [] as k}
                        <option value={k.hid}>{k.label}</option>
                      {/each}
                    </select>
                  {/each}
                {:else if entry.entry_type === 1}
                  <!-- Virtual: vaction slot index -->
                  <input type="number" min="0" max="31" style="width:5rem"
                         bind:value={entry.vaction_idx} />
                {:else}
                  <!-- Jump/Swap: no extra data needed -->
                  <span class="muted">—</span>
                {/if}
              </td>
              <td>
                <button class="outline secondary" onclick={() => removeEntry(i)}>✕</button>
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>

    <div class="actions">
      <button class="outline" onclick={addEntry}>+ Add entry</button>
      <button onclick={save} aria-busy={saving} disabled={saving}>Save all</button>
    </div>
  </section>
{:else if error}
  <p class="error">{error}</p>
{:else}
  <p aria-busy="true">Loading…</p>
{/if}

<style>
  .error  { color: var(--pico-color-red-500); }
  .muted  { color: var(--pico-muted-color); }
  .actions { display: flex; gap: 0.5rem; margin-top: 1rem; }
  .mod-grid { display: flex; flex-wrap: wrap; gap: 0.25rem; }
  .mod-label { display: flex; align-items: center; gap: 0.2rem; font-size: 0.8rem; }
  .mod-label input { margin: 0; }
</style>
