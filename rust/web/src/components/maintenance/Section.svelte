<script lang="ts">
  import type { Snippet } from 'svelte';

  let {
    title,
    columns = 1,
    children,
  }: {
    title: string;
    /** Two panels side by side on wide screens; they stretch to the same height. */
    columns?: 1 | 2;
    children: Snippet;
  } = $props();

  const id = $props.id();
</script>

<section class="group" aria-labelledby={id}>
  <h2 {id}>{title}</h2>
  <div class="panels" class:two={columns === 2}>
    {@render children()}
  </div>
</section>

<style>
  .group {
    display: grid;
    gap: var(--space-3);
    min-width: 0;
    container-type: inline-size;
  }
  h2 {
    font-family: var(--font);
    font-size: var(--text-sm);
    font-weight: var(--weight-semibold);
    /* On the page background, --muted is short of 4.5:1. */
    color: var(--ink-2);
  }
  .panels {
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    gap: var(--space-4);
  }
  @container (min-width: 900px) {
    .two {
      grid-template-columns: repeat(2, minmax(0, 1fr));
    }
  }
</style>
