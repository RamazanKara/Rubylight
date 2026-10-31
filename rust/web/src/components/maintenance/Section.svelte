<script lang="ts">
  import type { Snippet } from 'svelte';

  let {
    title,
    columns = 1,
    quiet = false,
    children,
  }: {
    title: string;
    /** The panels' own titles say it: the heading stays for screen readers only. */
    quiet?: boolean;
    /** Two panels side by side on wide screens; they stretch to the same height. */
    columns?: 1 | 2;
    children: Snippet;
  } = $props();

  const id = $props.id();
</script>

<section class="group" aria-labelledby={id}>
  <h2 {id} class:visually-hidden={quiet}>{title}</h2>
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
    font-family: var(--font-display);
    font-size: var(--text-md);
    font-weight: var(--weight-bold);
    line-height: var(--leading-tight);
    color: var(--ink);
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
