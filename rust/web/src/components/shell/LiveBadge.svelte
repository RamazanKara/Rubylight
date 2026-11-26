<script lang="ts">
  /**
   * What plays now, in the shell: a pill in the command bar that links to Home's live stage,
   * or just its beating dot beside Home in the phone's menu.
   */
  import { link } from '../../lib/router.svelte';
  import type { LiveBadge } from '../../lib/shell';

  interface Props {
    badge: LiveBadge;
    /** Only the dot: for the phone's menu, where the pill has no room. */
    dot?: boolean;
  }
  let { badge, dot = false }: Props = $props();
</script>

{#if dot}
  <span class="live-dot lone" class:streaming={badge.streaming} title={badge.title}></span>
{:else}
  <a class="live-badge" class:streaming={badge.streaming} href="/" use:link title={badge.title}>
    <span class="live-dot" class:streaming={badge.streaming} aria-hidden="true"></span>
    <span class="what">{badge.label}</span>
  </a>
{/if}

<style>
  .live-badge {
    display: inline-flex;
    align-items: center;
    gap: var(--space-2);
    min-height: var(--control-height);
    min-width: 0;
    padding-inline: var(--space-3) var(--space-4);
    border: 1px solid var(--line);
    border-radius: var(--radius-pill);
    background: var(--panel);
    box-shadow: var(--edge);
    color: var(--ink-2);
    font-size: var(--text-sm);
    font-weight: var(--weight-semibold);
    text-decoration: none;
    transition: border-color var(--duration-fast);
  }
  .live-badge:hover {
    border-color: var(--line-strong);
  }
  .live-badge.streaming {
    border-color: var(--ok-line);
    background: var(--ok-soft);
    color: var(--ok);
  }
  /* A long game title gives way to the search field, never the other way round. */
  .what {
    min-width: 0;
    max-width: 32ch;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  .live-dot {
    flex: none;
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--muted);
  }
  .live-dot.streaming {
    background: var(--live);
    animation: rubylight-live var(--pulse-duration) ease-in-out infinite;
  }
  .lone {
    margin-inline-start: auto;
  }
  @keyframes rubylight-live {
    50% {
      opacity: 0.35;
    }
  }
</style>
