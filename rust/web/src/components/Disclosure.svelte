<script lang="ts">
  import type { Snippet } from 'svelte';

  // The console's one fold: a labelled summary with the shared chevron
  // (base.css) at its end, pointing down when closed and up when open.
  // `card` is a whole folding card, its title a heading styled like a
  // panel's; `inline` is a short labelled fold inside text or a panel.
  let {
    title,
    description,
    meta,
    open = $bindable(false),
    variant = 'card',
    level,
    flush = false,
    id,
    ontoggle,
    children,
  }: {
    title: string;
    description?: string;
    /** Quiet facts after the title, such as "20 settings, 1 customized". */
    meta?: string;
    open?: boolean;
    variant?: 'card' | 'inline';
    /** The card title's heading level; it looks the same at either. */
    level?: 2 | 3;
    /** Content reaches the card's edges (setting rows, lists). */
    flush?: boolean;
    /** An id for links into the page, on the fold itself. */
    id?: string;
    ontoggle?: (open: boolean) => void;
    children: Snippet;
  } = $props();
</script>

<details
  class="disclosure {variant}"
  {id}
  bind:open
  ontoggle={(event) => ontoggle?.(event.currentTarget.open)}
>
  <summary>
    {#if variant === 'card'}
      <span class="head">
        {#if level}
          <svelte:element this={level === 3 ? 'h3' : 'h2'} class="title">{title}</svelte:element>
        {:else}
          <span class="title">{title}</span>
        {/if}
        {#if meta}<span class="meta">{meta}</span>{/if}
      </span>
      {#if description}<span class="description">{description}</span>{/if}
    {:else}
      {title}{#if meta}<span class="meta">{meta}</span>{/if}
    {/if}
  </summary>
  <div class="content" class:padded={variant === 'card' && !flush}>
    {@render children()}
  </div>
</details>

<style>
  .disclosure {
    min-width: 0;
    scroll-margin-top: var(--sticky-offset, var(--space-4));
  }
  .card {
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: var(--radius-lg);
    box-shadow: var(--shadow);
  }
  .card > summary {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    align-items: center;
    column-gap: var(--space-4);
    row-gap: 3px;
    padding: var(--space-4) var(--space-5);
    border-radius: var(--radius-lg);
    transition: background var(--duration-fast);
  }
  .card > summary:hover {
    background: var(--sunken);
  }
  /* The shared chevron, at the end of the row. */
  .card > summary::after {
    grid-column: 2;
    grid-row: 1 / span 2;
    margin-right: var(--space-1);
  }
  .card[open] > summary {
    border-bottom: 1px solid var(--line);
    border-radius: var(--radius-lg) var(--radius-lg) 0 0;
  }
  .head {
    grid-column: 1;
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 2px var(--space-3);
    min-width: 0;
  }
  /* The same heading as a panel's title, whatever the level. */
  .title {
    font-family: var(--font-display);
    font-size: var(--text-md);
    font-weight: var(--weight-bold);
    line-height: var(--leading-tight);
    color: var(--ink);
  }
  .meta {
    font-size: var(--text-sm);
    font-weight: var(--weight-regular);
    color: var(--muted);
  }
  .description {
    grid-column: 1;
    font-size: var(--text-sm);
    color: var(--muted);
  }
  .padded {
    padding: var(--space-5);
  }
  /* A short fold in running content: its label and chevron, a 44 px target
     that takes no more room than its line. */
  .inline > summary {
    display: inline-flex;
    align-items: center;
    gap: var(--space-2);
    width: fit-content;
    min-height: var(--control-height);
    margin-block: calc((20px - var(--control-height)) / 2);
    padding-block: 0;
    color: var(--ink-2);
    font-size: var(--text-sm);
    font-weight: var(--weight-medium);
  }
  .inline > summary::after {
    margin-left: 2px;
  }
  .inline > summary:hover {
    color: var(--ink);
  }
  .inline > .content {
    margin-top: var(--space-2);
  }
  @media (max-width: 520px) {
    .card > summary,
    .padded {
      padding: var(--space-4);
    }
  }
</style>
