<script>
  import { hid } from './lib/hid.js';
  import StatusBar from './routes/StatusBar.svelte';
  import VirtualActions from './routes/VirtualActions.svelte';
  import CapsLayer from './routes/CapsLayer.svelte';
  import KeyRemaps from './routes/KeyRemaps.svelte';
  import ConfigPage from './routes/ConfigPage.svelte';

  let page = $state('virtual-actions');
  let activeOutput = $state(0);

  hid.onOutputChange = (idx) => { activeOutput = idx; };

  const pages = [
    { id: 'virtual-actions', label: 'Virtual Actions' },
    { id: 'caps-layer',      label: 'Caps Layer'      },
    { id: 'key-remaps',      label: 'Key Remaps'      },
    { id: 'config',          label: 'Config'          },
  ];
</script>

<header class="container-fluid">
  <nav>
    <ul><li><strong>Dualie</strong></li></ul>
    <ul>
      {#each pages as p}
        <li>
          <a href="#{p.id}" class:active={page === p.id}
             onclick={(e) => { e.preventDefault(); page = p.id; }}>
            {p.label}
          </a>
        </li>
      {/each}
    </ul>
  </nav>
</header>

<main class="container">
  <StatusBar bind:activeOutput />

  {#if page === 'virtual-actions'}
    <VirtualActions {activeOutput} />
  {:else if page === 'caps-layer'}
    <CapsLayer {activeOutput} />
  {:else if page === 'key-remaps'}
    <KeyRemaps {activeOutput} />
  {:else if page === 'config'}
    <ConfigPage />
  {/if}
</main>

<style>
  nav a.active { font-weight: bold; text-decoration: underline; }
</style>
