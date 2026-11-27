<script lang="ts">
  /**
   * The saved clips section of the Clips page: the quota bar ("4.2 GB of 20 GB used", warning from
   * 90%), the clips folder, then the list or its empty state. The page loads and acts; this part
   * lays it out.
   */
  import Button from '../Button.svelte';
  import CopyButton from '../CopyButton.svelte';
  import EmptyState from '../EmptyState.svelte';
  import LoadState from '../LoadState.svelte';
  import Panel from '../Panel.svelte';
  import ClipsTable from './ClipsTable.svelte';
  import DvrMeter from './DvrMeter.svelte';
  import type { ClipSummary, ConsoleDvrStatus } from '../../lib/api';
  import { DVR_QUOTA_HREF, quotaNearFull, quotaShare, quotaText } from './dvr-view';

  let {
    clips,
    error = '',
    quota,
    dvr = null,
    more = false,
    deleting = null,
    loadingMore = false,
    onpreview,
    ondelete,
    onmore,
    onretry,
  }: {
    /** Newest first; null until the first load. */
    clips: ClipSummary[] | null;
    error?: string;
    quota: { used: number; limit: number };
    /** For the clips folder and whether Instant replay is on. */
    dvr?: ConsoleDvrStatus | null;
    more?: boolean;
    deleting?: string | null;
    loadingMore?: boolean;
    onpreview: (clip: ClipSummary) => void;
    ondelete: (clip: ClipSummary) => void;
    onmore: () => void;
    onretry: () => Promise<void>;
  } = $props();

  const nearFull = $derived(quotaNearFull(quota.used, quota.limit));
</script>

<Panel title="Saved clips" description={clips ? `${clips.length}${more ? '+' : ''} ${clips.length === 1 ? 'clip' : 'clips'}` : undefined} flush>
  {#if clips !== null || quota.limit}
    <div class="storage">
      <DvrMeter
        label="Space for clips"
        text={quotaText(quota.used, quota.limit)}
        share={quotaShare(quota.used, quota.limit)}
        tone={nearFull ? 'warn' : 'accent'}
      />
      {#if nearFull}
        <p class="notice warn">
          <span>Clips almost fill their space. The oldest replays are deleted to make room for new ones; recordings are kept.</span>
          <Button size="sm" href={DVR_QUOTA_HREF}>Change the space for clips</Button>
        </p>
      {/if}
      {#if dvr?.clips_dir}
        <p class="folder">
          <span class="muted">Folder</span>
          <code>{dvr.clips_dir}</code>
          <CopyButton value={dvr.clips_dir} label="Copy the clips folder" />
        </p>
        {#if !dvr.clips_dir_available}
          <p class="notice danger">This PC cannot reach the clips folder. Check that its drive is connected, or choose another folder in the settings.</p>
        {/if}
      {/if}
    </div>
  {/if}
  {#if clips === null}
    <div class="pad"><LoadState {error} what="the saved clips" {onretry} /></div>
  {:else}
    {#if error}<div class="pad"><LoadState {error} what="the saved clips" stale {onretry} /></div>{/if}
    {#if clips.length === 0}
      <EmptyState icon="film" title="No clips yet">
        <p>
          {#if dvr && !dvr.enabled}
            Turn on Instant Replay, then press Save clip here or in Nightjar's Control Center while you play.
          {:else}
            Press Save clip here or in Nightjar's Control Center while you play; clips and recordings show here.
          {/if}
        </p>
      </EmptyState>
    {:else}
      <ClipsTable
        {clips}
        {deleting}
        {more}
        {loadingMore}
        {onpreview}
        {ondelete}
        {onmore}
      />
    {/if}
  {/if}
</Panel>

<style>
  .storage {
    display: grid;
    gap: var(--space-3);
    padding: var(--space-4) var(--space-5);
    border-bottom: 1px solid var(--line);
  }
  .storage .notice {
    margin: 0;
    flex-wrap: wrap;
  }
  /* The quota link keeps its width and drops below the sentence when the row is narrow. */
  .storage .notice > span {
    flex: 1 1 28ch;
  }
  .storage .notice :global(.button) {
    flex: none;
  }
  .folder {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-2);
    margin: 0;
    font-size: var(--text-sm);
    min-width: 0;
  }
  .folder code {
    overflow-wrap: anywhere;
    min-width: 0;
  }
  .pad {
    padding: var(--space-4) var(--space-5);
  }
  @media (max-width: 520px) {
    .storage,
    .pad {
      padding-inline: var(--space-4);
    }
  }
</style>
