<script lang="ts">
  import { onMount, untrack } from 'svelte';
  import Banner from '../Banner.svelte';
  import Button from '../Button.svelte';
  import Icon from '../Icon.svelte';
  import LoadState from '../LoadState.svelte';
  import Panel from '../Panel.svelte';
  import StatusPill from '../StatusPill.svelte';
  import { api, type CoopStatus } from '../../lib/api';
  import { EARLIER_COPY_GAMES, folderTitle, nucleusRelease, sentence } from '../../lib/coop-view';
  import { settingLink } from '../../lib/quick-switch';
  import { installPill } from './coop';
  import { statusLoader } from './status-loader';
  import { reason } from './util';

  /**
   * Troubleshoot's co-op status: where Nucleus Co-op runs from, and why it can't. Its folder has one
   * home, the Nucleus Co-op folder setting (a game's Play together panel sets it up in place), which
   * this panel links to. Live games are on Home.
   */
  let { initial = null }: { initial?: CoopStatus | null } = $props();

  let status = $state<CoopStatus | null>(untrack(() => initial));
  let loadError = $state('');

  const install = $derived(status ? installPill(status.install) : null);
  const release = $derived(status ? nucleusRelease(status) : null);
  /** Nothing usable is in place: the release's page is the next step. */
  const needsDownload = $derived(status?.install.state === 'missing' || status?.install.state === 'failed');
  const folderLink = settingLink('nucleus_path');

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

<Panel title="Nucleus Co-op" level={3} description="Co-op games, and games played while someone uses this PC.">
  {#snippet actions()}
    {#if install}<StatusPill status={install.status} label={install.label} />{/if}
  {/snippet}
  {#if status && release}
    <div class="stack">
      {#if status.install.version || status.install.path || status.install.chosen}
        <dl class="facts">
          {#if status.install.version}
            <dt>Version</dt>
            <dd>{status.install.version}</dd>
          {/if}
          {#if status.install.path}
            <dt>Runs from</dt>
            <dd class="mono">{status.install.path}</dd>
          {/if}
          {#if status.install.chosen && status.install.chosen !== status.install.path}
            <dt>Folder chosen</dt>
            <dd class="mono">{status.install.chosen}</dd>
          {/if}
        </dl>
      {/if}
      {#if !status.available && status.reason}
        <Banner tone="warn" title="Co-op can’t run on this PC">{sentence(status.reason)}</Banner>
      {/if}
      {#if status.install.state === 'failed' && status.install.error}
        <Banner tone="danger" title={folderTitle(status.install.problem)} live>{sentence(status.install.error)}</Banner>
      {/if}
      {#if status.install.notice}
        <Banner tone="warn" title="Rubylight runs the earlier copy">{sentence(status.install.notice)} {EARLIER_COPY_GAMES}</Banner>
      {/if}
      {#if status.catalog_error}
        <Banner tone="danger" title="Nucleus Co-op’s handlers didn’t load">{sentence(status.catalog_error)}</Banner>
      {/if}
      <div class="row">
        {#if needsDownload}
          <a class="button-link" href={release.page} target="_blank" rel="noreferrer">
            Download Nucleus Co-op {release.version}<Icon name="external" size={14} />
          </a>
        {/if}
        <Button href={folderLink} icon="folder" variant={status.install.state === 'missing' ? 'primary' : 'secondary'}>
          {status.install.chosen ? 'Change folder' : 'Choose folder'}
        </Button>
      </div>
    </div>
  {:else}
    <LoadState error={loadError} what="the co-op status" onretry={load} />
  {/if}
</Panel>

<style>
  .row {
    gap: var(--space-2) var(--space-4);
  }
  .button-link {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    min-height: var(--control-height);
    font-size: var(--text-sm);
    font-weight: var(--weight-medium);
  }
</style>
