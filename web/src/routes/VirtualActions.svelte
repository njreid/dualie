<script>
  import { onMount } from 'svelte';
  import { getActions, putAction, getPlatformApps } from '../lib/api.js';

  let { activeOutput } = $props();

  let actions = $state([]);
  let apps    = $state([]);
  let error   = $state('');
  let saving  = $state(-1);

  onMount(async () => {
    try {
      [actions, apps] = await Promise.all([
        getActions(activeOutput),
        getPlatformApps(),
      ]);
    } catch (e) {
      error = e.message;
    }
  });

  // Reload when output tab switches
  $effect(() => {
    activeOutput;
    getActions(activeOutput)
      .then((a) => { actions = a; })
      .catch((e) => { error = e.message; });
  });

  async function save(slot) {
    saving = slot;
    error = '';
    try {
      await putAction(activeOutput, slot, actions[slot]);
    } catch (e) {
      error = e.message;
    } finally {
      saving = -1;
    }
  }

  function setType(slot, type) {
    if (type === 'unset') {
      actions[slot] = { type: 'unset' };
    } else if (type === 'app_launch') {
      actions[slot] = { type: 'app_launch', app_id: '', label: '' };
    } else if (type === 'shell_command') {
      actions[slot] = { type: 'shell_command', command: '', label: '' };
    }
  }

  function pickApp(slot, app_id) {
    const app = apps.find((a) => a.id === app_id);
    if (app) {
      actions[slot].app_id = app.id;
      actions[slot].label  = app.name;
    }
  }
</script>

<section>
  <h2>Virtual Actions — Output {activeOutput === 0 ? 'A' : 'B'}</h2>
  <p>Map virtual key slots (F13–F24 etc.) to actions dispatched by the daemon.</p>

  {#if error}<p class="error">{error}</p>{/if}

  <div class="table-container">
    <table role="grid">
      <thead>
        <tr><th>Slot</th><th>Key</th><th>Type</th><th>Target</th><th></th></tr>
      </thead>
      <tbody>
        {#each actions as action, slot}
          {@const vkeys = ['F13','F14','F15','F16','F17','F18','F19','F20','F21','F22','F23','F24',
                           'Execute','Help','Menu','Select','Stop','Again','Undo','Cut',
                           'Intl1','Intl2','Intl3','Intl4','Intl5','Intl6','Intl7','Intl8',
                           'Lang1','Lang2','Lang3','Lang4']}
          <tr>
            <td>{slot}</td>
            <td><kbd>{vkeys[slot] ?? slot}</kbd></td>
            <td>
              <select value={action.type}
                      onchange={(e) => setType(slot, e.target.value)}>
                <option value="unset">Unset</option>
                <option value="app_launch">App Launch</option>
                <option value="shell_command">Shell Command</option>
              </select>
            </td>
            <td>
              {#if action.type === 'app_launch'}
                <select value={action.app_id}
                        onchange={(e) => pickApp(slot, e.target.value)}>
                  <option value="">— choose app —</option>
                  {#each apps as app}
                    <option value={app.id}>{app.name}</option>
                  {/each}
                </select>
              {:else if action.type === 'shell_command'}
                <input type="text" placeholder="shell command…"
                       bind:value={action.command} />
              {:else}
                <span class="muted">—</span>
              {/if}
            </td>
            <td>
              <button onclick={() => save(slot)}
                      disabled={saving === slot}
                      aria-busy={saving === slot}
                      class="outline">
                Save
              </button>
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
</section>

<style>
  .error { color: var(--pico-color-red-500); }
  .muted { color: var(--pico-muted-color); }
  select, input { margin: 0; }
</style>
