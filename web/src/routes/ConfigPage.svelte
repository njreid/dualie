<script>
  import { downloadConfig, uploadConfig, getPlatformInfo } from '../lib/api.js';
  import { onMount } from 'svelte';

  let info      = $state(null);
  let error     = $state('');
  let uploading = $state(false);
  let fileInput = $state(null);

  onMount(async () => {
    try {
      info = await getPlatformInfo();
    } catch (e) {
      error = e.message;
    }
  });

  async function handleUpload(e) {
    const file = e.target.files?.[0];
    if (!file) return;
    uploading = true;
    error = '';
    try {
      await uploadConfig(file);
      // Reload the page so all panels reflect the new config
      location.reload();
    } catch (e) {
      error = e.message;
    } finally {
      uploading = false;
    }
  }
</script>

<section>
  <h2>Config</h2>

  {#if info}
    <article>
      <header>System</header>
      <dl>
        <dt>OS</dt>   <dd>{info.os}</dd>
        <dt>Arch</dt> <dd>{info.arch}</dd>
        <dt>Version</dt><dd>{info.version || '—'}</dd>
      </dl>
    </article>
  {/if}

  {#if error}<p class="error">{error}</p>{/if}

  <article>
    <header>Backup &amp; Restore</header>
    <p>Download the current configuration as a CBOR file, or restore a previously saved backup.</p>
    <div class="actions">
      <button onclick={downloadConfig}>Download config</button>

      <button aria-busy={uploading} disabled={uploading}
              onclick={() => fileInput.click()}>
        {uploading ? 'Uploading…' : 'Upload config'}
      </button>
      <input bind:this={fileInput} type="file" accept=".cbor"
             onchange={handleUpload} style="display:none" disabled={uploading} />
    </div>
  </article>
</section>

<style>
  .error   { color: var(--pico-color-red-500); }
  .actions { display: flex; gap: 0.5rem; flex-wrap: wrap; align-items: center; }
  dt { font-weight: bold; }
  dd { margin-left: 1rem; margin-bottom: 0.5rem; }
</style>
