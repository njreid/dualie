<script>
  import { hid } from '../lib/hid.js';

  let { activeOutput = $bindable(0) } = $props();

  let connected  = $state(false);
  let connecting = $state(false);
  let pushing    = $state(false);
  let error      = $state('');

  async function connect() {
    connecting = true;
    error = '';
    try {
      await hid.connect();
      connected = true;
    } catch (e) {
      error = e.message;
    } finally {
      connecting = false;
    }
  }

  async function disconnect() {
    await hid.disconnect();
    connected = false;
  }

  async function switchOutput(idx) {
    try {
      await hid.switchOutput(idx);
      activeOutput = idx;
    } catch (e) {
      error = e.message;
    }
  }

  async function pushConfig() {
    pushing = true;
    error = '';
    try {
      await hid.pushConfig();
    } catch (e) {
      error = e.message;
    } finally {
      pushing = false;
    }
  }
</script>

<article>
  <header>
    <strong>Device</strong>
    {#if connected}
      <span> · Connected ·</span>
      <button class="outline secondary" onclick={disconnect}>Disconnect</button>
      <button onclick={pushConfig} aria-busy={pushing} disabled={pushing} class="outline">
        {pushing ? 'Pushing…' : 'Push to device'}
      </button>
      <span>
        Output:
        <button class={activeOutput === 0 ? '' : 'outline'} onclick={() => switchOutput(0)}>A</button>
        <button class={activeOutput === 1 ? '' : 'outline'} onclick={() => switchOutput(1)}>B</button>
      </span>
    {:else}
      <button onclick={connect} aria-busy={connecting} disabled={connecting}>
        {connecting ? 'Connecting…' : 'Connect via WebHID'}
      </button>
    {/if}
  </header>
  {#if error}<p class="error">{error}</p>{/if}
</article>

<style>
  article header { display: flex; align-items: center; gap: 0.75rem; flex-wrap: wrap; }
  article header strong { flex: 1; }
  .error { color: var(--pico-color-red-500); margin: 0; }
</style>
