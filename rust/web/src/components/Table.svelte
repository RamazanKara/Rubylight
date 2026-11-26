<script lang="ts">
  // A data table that scrolls sideways inside its own region on narrow screens
  // instead of widening the page. Children are the <thead> and <tbody>. An edge that hides
  // columns fades out, so a phone shows there is more to scroll to.
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

  let region = $state<HTMLElement>();
  /** Columns wait beyond the start or the end of the region. */
  let more = $state({ start: false, end: false });

  function measure() {
    if (!region) return;
    const hidden = region.scrollWidth - region.clientWidth;
    const at = Math.abs(region.scrollLeft);
    more = { start: at > 1, end: at < hidden - 1 };
  }

  $effect(() => {
    if (!region || typeof ResizeObserver === 'undefined') return;
    const observer = new ResizeObserver(measure);
    observer.observe(region);
    if (region.firstElementChild) observer.observe(region.firstElementChild);
    measure();
    return () => observer.disconnect();
  });
</script>

<!-- A scrolling region must be reachable by keyboard to scroll it (axe scrollable-region-focusable). -->
<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
<div class="scroll" class:more-start={more.start} class:more-end={more.end} role="region" aria-label={caption} tabindex="0" bind:this={region} onscroll={measure}>
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
  /* The scroll hint: the edge that hides columns fades out. */
  .more-end {
    mask-image: linear-gradient(to left, transparent, var(--mask-solid) var(--space-7));
  }
  .more-start {
    mask-image: linear-gradient(to right, transparent, var(--mask-solid) var(--space-7));
  }
  .more-start.more-end {
    mask-image: linear-gradient(to right, transparent, var(--mask-solid) var(--space-7), var(--mask-solid) calc(100% - var(--space-7)), transparent);
  }
  caption {
    text-align: start;
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
    text-align: end;
    font-variant-numeric: tabular-nums;
    /* A number and its unit stay together ("48.2 Mbps"). */
    white-space: nowrap;
  }
</style>
