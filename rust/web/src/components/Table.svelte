<script lang="ts">
  // A data table that scrolls sideways inside its own region on narrow screens
  // instead of widening the page. Children are the <thead> and <tbody>.
  import type { Snippet } from 'svelte';

  let {
    caption,
    showCaption = false,
    dense = false,
    children,
  }: {
    /** Names the table; hidden unless showCaption. */
    caption: string;
    showCaption?: boolean;
    dense?: boolean;
    children: Snippet;
  } = $props();
</script>

<!-- A scrolling region must be reachable by keyboard to scroll it (axe scrollable-region-focusable). -->
<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
<div class="scroll" role="region" aria-label={caption} tabindex="0">
  <table class="table" class:dense>
    <caption class:visually-hidden={!showCaption}>{caption}</caption>
    {@render children()}
  </table>
</div>

<style>
  .scroll {
    overflow-x: auto;
    min-width: 0;
  }
  .scroll:focus-visible {
    outline-offset: calc(var(--focus-width) * -1);
  }
  caption {
    text-align: left;
    padding: var(--space-3) var(--space-5);
    font-weight: var(--weight-semibold);
  }
  .table :global(tbody tr:hover) {
    background: var(--sunken);
  }
  .dense :global(th),
  .dense :global(td) {
    padding-block: var(--space-1);
  }
  .table :global(.numeric) {
    text-align: right;
    font-variant-numeric: tabular-nums;
  }
</style>
