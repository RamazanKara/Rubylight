<script lang="ts">
  import { onMount, untrack } from 'svelte';
  import Banner from '../Banner.svelte';
  import LoadState from '../LoadState.svelte';
  import Panel from '../Panel.svelte';
  import NucleusFolder from '../library/NucleusFolder.svelte';
  import { api, type CoopStatus } from '../../lib/api';
  import { folderTitle, sentence } from '../../lib/coop-view';
  import { link } from '../../lib/router.svelte';
  import StatusPill from '../StatusPill.svelte';
  import { installPill } from './coop';
  import { statusLoader } from './status-loader';
  import { reason } from './util';

  /**
   * Co-op troubleshooting on Help: Nucleus Co-op's folder, why it can't run from it, the service and
   * the Windows build. Live games are on Home.
   */
  let { initial = null }: { initial?: CoopStatus | null } = $props();

  let status = $state<CoopStatus | null>(untrack(() => initial));
  let loadError = $state('');

  const install = $derived(status ? installPill(status.install) : null);

  const loader = statusLoader(
    () => api.coop.status(),
    (value) => {
      status = value;
      loadError = '';
    },
    (error) => {
      loadError = reason(error);
    },
  );
  const load = loader.load;

  onMount(() => {
    if (!status) load();
  });
</script>

<Panel title="Nucleus Co-op" level={3} description="Runs co-op games, and games played while someone uses this PC.">
  {#if status}
    <div class="stack">
      <dl class="facts">
        <dt>Install</dt>
        <dd>
          {#if install}<StatusPill status={install.status} label={install.label} />{/if}
          {#if status.install.version}<span class="muted">{status.install.version}</span>{/if}
        </dd>
        {#if status.install.path}
          <dt>Folder</dt>
          <dd class="mono path">{status.install.path}</dd>
        {/if}
      </dl>
      {#if !status.available && status.reason}
        <Banner tone="warn" title="Co-op can’t run on this PC">{sentence(status.reason)}</Banner>
      {/if}
      {#if status.install.state === 'failed' && status.install.error}
        <Banner tone="danger" title={folderTitle(status.install.problem)} live>{sentence(status.install.error)}</Banner>
      {/if}
      <NucleusFolder {status} onchosen={load} />
      {#if status.catalog_error}
        <Banner tone="danger" title="Nucleus Co-op’s handlers didn’t load">{sentence(status.catalog_error)}</Banner>
      {/if}
      <ul class="hint">
        <li>
          Rubylight downloads nothing: it runs only Nucleus Co-op {status.install.release?.version ?? '2.4.2'}, checks its NucleusCoop.exe, and
          refuses a folder anyone but administrators may change, since Nucleus Co-op runs elevated.
        </li>
        <li>Co-op needs Rubylight running as its Windows service, which starts Nucleus Co-op as the signed-in user.</li>
        <li>
          Co-op needs a handler from Nucleus Co-op’s handler hub for each game; a background stream of one player works
          without one. See what a game still needs on its page under <a href="/library" use:link>Games</a>.
        </li>
        <li>Each player hears only their own game on Windows build 20348 or newer; older builds say so on the stream.</li>
      </ul>
    </div>
  {:else}
    <LoadState error={loadError} what="the co-op status" onretry={load} />
  {/if}
</Panel>

<style>
  .hint {
    margin: 0;
    padding-left: var(--space-5);
    display: grid;
    gap: var(--space-1);
    font-size: var(--text-sm);
    color: var(--ink-2);
  }
  .path {
    overflow-wrap: anywhere;
  }
</style>
