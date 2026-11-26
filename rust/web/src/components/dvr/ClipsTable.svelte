<script lang="ts">
  /**
   * The saved clips, newest first: game, when, length, picture (codec, HDR), size and kind, with
   * Preview, Download and Delete on each row and Show more for the next page. The page owns the
   * list and the actions; this part lays them out.
   */
  import Badge from '../Badge.svelte';
  import Button from '../Button.svelte';
  import Table from '../Table.svelte';
  import { clipDownloadUrl, type ClipSummary } from '../../lib/api';
  import { clipLength, clipName, codecLabel, kindLabel, pictureText, savedAt, size } from './dvr-view';

  let {
    clips,
    deleting = null,
    more = false,
    loadingMore = false,
    onpreview,
    ondelete,
    onmore,
  }: {
    clips: ClipSummary[];
    /** The id of the clip being deleted. */
    deleting?: string | null;
    /** The host has older clips past this page. */
    more?: boolean;
    loadingMore?: boolean;
    onpreview: (clip: ClipSummary) => void;
    ondelete: (clip: ClipSummary) => void;
    onmore: () => void;
  } = $props();
</script>

<Table caption="Saved clips, newest first">
  <thead>
    <tr>
      <th>Clip</th>
      <th>Saved</th>
      <th class="numeric">Length</th>
      <th>Picture</th>
      <th class="numeric">Size</th>
      <th><span class="visually-hidden">Actions</span></th>
    </tr>
  </thead>
  <tbody>
    {#each clips as clip (clip.id)}
      <tr>
        <td class="clip">
          <span class="title">{clipName(clip)}</span>
          <span class="tags">
            <Badge tone={clip.kind === 'manual' ? 'accent' : 'neutral'}>{kindLabel(clip.kind)}</Badge>
            {#if clip.recovered}<Badge tone="warn">Recovered</Badge>{/if}
          </span>
        </td>
        <td class="when">{savedAt(clip.created_unix_ms)}</td>
        <td class="numeric num">{clipLength(clip.duration_ms)}</td>
        <td class="picture">
          <span class="tags">
            <Badge>{codecLabel(clip.codec)}</Badge>
            {#if clip.hdr}<Badge tone="accent">HDR</Badge>{/if}
          </span>
          <span class="sub num">{pictureText(clip)}</span>
        </td>
        <td class="numeric num">{size(clip.size_bytes)}</td>
        <td class="row-actions">
          <Button size="sm" variant="ghost" icon="play" onclick={() => onpreview(clip)}>
            Preview<span class="visually-hidden">{` ${clipName(clip)}`}</span>
          </Button>
          <Button size="sm" variant="ghost" icon="download" href={clipDownloadUrl(clip.id)}>
            Download<span class="visually-hidden">{` ${clipName(clip)}`}</span>
          </Button>
          <Button size="sm" variant="ghost" icon="trash" busy={deleting === clip.id} onclick={() => ondelete(clip)}>
            Delete<span class="visually-hidden">{` ${clipName(clip)}`}</span>
          </Button>
        </td>
      </tr>
    {/each}
  </tbody>
</Table>
{#if clips.some((clip) => clip.recovered)}
  <p class="footnote muted">Recovered clips were kept from a recording this PC could not finish, such as when it shut down mid-recording.</p>
{/if}
{#if more}
  <div class="more">
    <Button busy={loadingMore} onclick={onmore}>Show older clips</Button>
  </div>
{/if}

<style>
  th,
  td {
    white-space: nowrap;
  }
  .clip {
    min-width: 18ch;
    white-space: normal;
  }
  .title {
    display: block;
    font-weight: var(--weight-medium);
  }
  .tags {
    display: inline-flex;
    flex-wrap: wrap;
    gap: var(--space-1);
    margin-top: var(--space-1);
  }
  .picture .tags {
    margin-top: 0;
  }
  .sub {
    display: block;
    margin-top: var(--space-1);
    font-size: var(--text-xs);
    color: var(--muted);
  }
  .row-actions {
    text-align: end;
  }
  .row-actions :global(.button + .button) {
    margin-inline-start: var(--space-1);
  }
  .footnote {
    margin: 0;
    padding: var(--space-3) var(--space-5);
    border-top: 1px solid var(--line);
    font-size: var(--text-xs);
  }
  .more {
    display: flex;
    justify-content: center;
    padding: var(--space-4);
    border-top: 1px solid var(--line);
  }
</style>
