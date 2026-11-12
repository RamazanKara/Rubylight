<script lang="ts">
  import type { Snippet } from 'svelte';
  import Icon from '../Icon.svelte';
  import { link } from '../../lib/router.svelte';

  let {
    title,
    columns = 1,
    quiet = false,
    anchor,
    more,
    children,
  }: {
    title: string;
    /** An id for links into the page (/maintenance#host), on the section itself so it adds no gap. */
    anchor?: string;
    /** The panels' own titles say it: the heading stays for screen readers only. */
    quiet?: boolean;
    /** Two panels side by side on wide screens, each as tall as its content. */
    columns?: 1 | 2;
    /** Where the rest of this topic is: a quiet link beside the heading. */
    more?: { href: string; label: string };
    children: Snippet;
  } = $props();

  const id = $props.id();
</script>

<section class="group" id={anchor} aria-labelledby={id}>
  {#if more && !quiet}
    <div class="heading">
      <h2 {id}>{title}</h2>
      <a class="more" href={more.href} use:link>{more.label}<span class="visually-hidden"> for {title}</span><Icon name="chevron" size={14} /></a>
    </div>
  {:else}
    <h2 {id} class:visually-hidden={quiet}>{title}</h2>
  {/if}
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
  /* A section heading: above its panels' titles (Panel.svelte), below the page title. */
  h2 {
    font-family: var(--font-display);
    font-size: var(--text-lg);
    font-weight: var(--weight-bold);
    line-height: var(--leading-tight);
    color: var(--ink);
  }
  .heading {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    flex-wrap: wrap;
    gap: var(--space-1) var(--space-4);
  }
  .more {
    display: inline-flex;
    align-items: center;
    gap: var(--space-1);
    min-height: var(--control-height);
    margin-block: calc((22px - var(--control-height)) / 2);
    border-radius: var(--radius-sm);
    color: var(--ink-2);
    font-size: var(--text-sm);
    font-weight: var(--weight-medium);
    text-decoration: none;
  }
  .more:hover {
    color: var(--ink);
    text-decoration: underline;
  }
  .panels {
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    gap: var(--space-4);
    align-items: start;
  }
  @container (min-width: 900px) {
    .two {
      grid-template-columns: repeat(2, minmax(0, 1fr));
    }
  }
</style>
