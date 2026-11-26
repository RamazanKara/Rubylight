<script lang="ts">
  /**
   * The display map: each display the host lists, drawn where it sits on the Windows desktop,
   * with its mode, scale, HDR and role in the list beside it.
   *
   * Read-only by design. Arranging displays is Windows' job, and the only layout Rubylight writes
   * is the saved one it restores after a stream (Troubleshoot, Displays), which asks first.
   */
  import Badge from '../Badge.svelte';
  import EmptyState from '../EmptyState.svelte';
  import LoadState from '../LoadState.svelte';
  import Skeleton from '../Skeleton.svelte';
  import type { DisplayDevice } from '../../lib/api';
  import { describe, topology } from './display-topology';

  let {
    displays,
    error = '',
    onretry,
    target,
    targetLabel = 'Streams here',
  }: {
    /** `null` while the list loads. */
    displays: readonly DisplayDevice[] | null;
    error?: string;
    onretry?: () => Promise<void>;
    /** The display a stream is set to use: its id or its Windows name. */
    target?: string;
    targetLabel?: string;
  } = $props();

  const map = $derived(displays ? topology(displays, target) : null);
  /** The display the pointer or the keyboard is on, lit on the map and in the list. */
  let lit = $state('');
</script>

<div class="topology">
  {#if !map}
    {#if error && onretry}
      <LoadState {error} what="the displays" {onretry} />
    {:else if error}
      <p class="note" role="status">The displays could not be read: {error}</p>
    {:else}
      <div class="loading"><Skeleton shape="block" height="140px" label="Loading the displays" /></div>
    {/if}
  {:else if !map.tiles.length}
    <EmptyState icon="monitor" title="No display is connected">
      Rubylight creates a virtual display when a device starts a stream, if virtual displays are on.
    </EmptyState>
  {:else}
    {#if error}
      <p class="note" role="status">This may be out of date; the displays could not be read again: {error}</p>
    {/if}
    <div class="stage">
      <div class="map" class:row={!map.spatial} style:--ratio={map.ratio} aria-hidden="true">
        {#each map.tiles as tile (tile.id)}
          <div
            class="tile"
            class:primary={tile.primary}
            class:virtual={tile.virtual}
            class:target={tile.target}
            class:lit={lit === tile.id}
            style:left="{tile.left}%"
            style:top="{tile.top}%"
            style:width="{tile.width}%"
            style:height="{tile.height}%"
          >
            <span class="screen">
              <span class="number">{tile.index}</span>
              {#if tile.primary}<span class="star">★</span>{/if}
            </span>
          </div>
        {/each}
      </div>
    </div>
    <ul class="list" aria-label="Displays">
      {#each map.tiles as tile (tile.id)}
        <li
          class:lit={lit === tile.id}
          class:target={tile.target}
          onpointerenter={() => (lit = tile.id)}
          onpointerleave={() => (lit = '')}
        >
          <span class="number" aria-hidden="true">{tile.index}</span>
          <span class="who">
            <span class="name" title={tile.name}>{tile.name}</span>
            <span class="facts">
              {#if tile.mode}<span class="mode">{tile.mode}</span>{/if}
              {#if tile.place && !tile.primary}<span>{tile.place}</span>{/if}
            </span>
            <span class="visually-hidden">{describe(tile)}</span>
          </span>
          <span class="pills" aria-hidden="true">
            {#if tile.target}<Badge tone="accent">{targetLabel}</Badge>{/if}
            {#if tile.primary}<Badge>Primary ★</Badge>{/if}
            {#if tile.scale}<Badge>{tile.scale}</Badge>{/if}
            {#if tile.hdr}<Badge tone={tile.hdr === 'on' ? 'ok' : 'neutral'}>HDR {tile.hdr}</Badge>{/if}
            <Badge>{tile.virtual ? 'Virtual' : 'Physical'}</Badge>
          </span>
        </li>
      {/each}
    </ul>
    <p class="note">
      {#if map.spatial}
        Drawn as Windows arranges the displays. To move one, use Windows’ display settings.
      {:else}
        Windows did not report where these displays sit, so they are listed in order.
      {/if}
    </p>
  {/if}
</div>

<style>
  .topology {
    display: grid;
    gap: var(--space-3);
    min-width: 0;
  }
  .stage {
    display: grid;
    place-items: center;
    padding: var(--space-5) var(--space-4);
    border: 1px solid var(--line);
    border-radius: var(--radius-lg);
    background: var(--sunken);
    box-shadow: var(--edge);
    min-width: 0;
  }
  /* As wide as the panel allows, never taller than 200 px: the ratio is the desktop's own. */
  .map {
    position: relative;
    width: min(100%, calc(200px * var(--ratio)));
    aspect-ratio: var(--ratio);
  }
  .map.row {
    width: min(100%, calc(96px * var(--ratio)));
  }
  .tile {
    position: absolute;
    display: grid;
    padding: 2px;
    min-width: 0;
  }
  .screen {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: var(--space-1);
    min-width: 0;
    overflow: hidden;
    border: 1px solid var(--line-strong);
    border-radius: var(--radius-sm);
    background: var(--panel);
    box-shadow: var(--shadow);
    color: var(--ink-2);
    font-size: var(--text-md);
    font-weight: var(--weight-semibold);
    font-variant-numeric: tabular-nums;
    transition:
      border-color var(--duration-fast) var(--ease-standard),
      background-color var(--duration-fast) var(--ease-standard);
  }
  .tile.primary .screen {
    color: var(--ink);
  }
  .tile.virtual .screen {
    border-style: dashed;
  }
  .tile.target .screen {
    border-color: var(--accent-line);
    background: var(--accent-soft);
    color: var(--accent-text);
  }
  .tile.lit .screen {
    border-color: var(--accent);
  }
  .star {
    font-size: var(--text-xs);
    color: var(--accent-text);
  }
  .list {
    display: grid;
    margin: 0;
    padding: 0;
    list-style: none;
    border: 1px solid var(--line);
    border-radius: var(--radius-lg);
    background: var(--panel);
    min-width: 0;
  }
  li {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr) auto;
    align-items: center;
    gap: var(--space-3);
    padding: var(--space-3) var(--space-4);
    min-width: 0;
    transition: background-color var(--duration-fast) var(--ease-standard);
  }
  li + li {
    border-top: 1px solid var(--line);
  }
  li.lit {
    background: var(--field);
  }
  li .number {
    display: grid;
    place-items: center;
    width: 28px;
    height: 28px;
    border: 1px solid var(--line-strong);
    border-radius: var(--radius-sm);
    background: var(--sunken);
    color: var(--ink-2);
    font-size: var(--text-sm);
    font-weight: var(--weight-semibold);
    font-variant-numeric: tabular-nums;
  }
  li.target .number {
    border-color: var(--accent-line);
    background: var(--accent-soft);
    color: var(--accent-text);
  }
  .who {
    display: grid;
    gap: 2px;
    min-width: 0;
  }
  .name {
    overflow: hidden;
    color: var(--ink);
    font-size: var(--text-sm);
    font-weight: var(--weight-medium);
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .facts {
    display: flex;
    flex-wrap: wrap;
    gap: 0 var(--space-3);
    color: var(--muted);
    font-size: var(--text-xs);
    line-height: 1.45;
  }
  .mode {
    font-variant-numeric: tabular-nums;
  }
  .pills {
    display: flex;
    flex-wrap: wrap;
    justify-content: flex-end;
    gap: var(--space-1);
  }
  .note {
    color: var(--muted);
    font-size: var(--text-xs);
    line-height: 1.45;
  }
  @media (max-width: 560px) {
    li {
      grid-template-columns: auto minmax(0, 1fr);
    }
    .pills {
      grid-column: 2;
      justify-content: flex-start;
    }
  }
</style>
