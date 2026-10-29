<script lang="ts">
  import { onMount, untrack } from 'svelte';
  import Badge from '../Badge.svelte';
  import Banner from '../Banner.svelte';
  import Button from '../Button.svelte';
  import LoadState from '../LoadState.svelte';
  import Panel from '../Panel.svelte';
  import { api, type CoopStatus } from '../../lib/api';
  import { downloadText, sentence } from '../../lib/coop-view';
  import { failed } from '../../lib/feedback.svelte';
  import { poll } from '../../lib/format';
  import { link } from '../../lib/router.svelte';
  import { installStates } from './coop';
  import { statusLoader } from './status-loader';
  import { reason } from './util';

  /**
   * Co-op troubleshooting on Help: the install, the download's check, the service and the
   * Windows build. Setting co-op up lives on each game's page; live games on Home.
   */
  let { initial = null }: { initial?: CoopStatus | null } = $props();

  let status = $state<CoopStatus | null>(untrack(() => initial));
  let loadError = $state('');
  let retrying = $state(false);

  const install = $derived(status ? installStates[status.install.state] : null);

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

  async function retry() {
    retrying = true;
    try {
      await api.coop.install();
    } catch (error) {
      failed('Setting up co-op failed', error);
    } finally {
      retrying = false;
    }
    await load();
  }

  onMount(() => {
    if (!status) load();
    return poll(() => {
      if (status?.install.state === 'installing') return loader.tick();
    }, 3000);
  });
</script>

<Panel title="Nucleus Co-op" level={3} description="Runs co-op games, and games played while someone uses this PC.">
  {#if status}
    <div class="stack">
      <dl class="facts">
        <dt>Install</dt>
        <dd>
          {#if install}<Badge tone={install.tone}>{install.label}</Badge>{/if}
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
        <Banner tone="danger" title="Setting up co-op failed" live>
          {sentence(status.install.error)}
          {#snippet actions()}<Button size="sm" busy={retrying} onclick={retry}>Try again</Button>{/snippet}
        </Banner>
      {/if}
      {#if status.catalog_error}
        <Banner tone="danger" title="Nucleus Co-op’s handlers didn’t load">{sentence(status.catalog_error)}</Banner>
      {/if}
      <ul class="hint">
        <li>{downloadText(status)} Its size and SHA-256 are checked; a download that doesn’t match is deleted and refused.</li>
        <li>Co-op needs Rubylight running as its Windows service, which starts Nucleus Co-op as the signed-in user.</li>
        <li>
          Co-op needs a handler from Nucleus Co-op’s handler hub for each game; a background stream of one player works
          without one. Set co-op up, and see what a game still needs, on its page under <a href="/library" use:link>Games</a>.
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
