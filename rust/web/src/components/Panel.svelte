<script lang="ts">
  import type { Snippet } from 'svelte';

  let {
    title,
    description,
    actions,
    flush = false,
    collapsible = false,
    open = false,
    level = 2,
    children,
  }: {
    title?: string;
    description?: string;
    actions?: Snippet;
    /** Content reaches the panel's edges (tables, lists). */
    flush?: boolean;
    collapsible?: boolean;
    open?: boolean;
    /** Heading level of the title: 3 inside a titled page section. It looks the same. */
    level?: 2 | 3;
    children: Snippet;
  } = $props();
</script>

<section class="panel">
  {#if collapsible}
    <details {open}>
      <summary><span class="title">{title}</span>{#if description}<span class="muted description">{description}</span>{/if}</summary>
      <div class:body={!flush}>{@render children()}</div>
    </details>
  {:else}
  {#if title || actions}
    <header>
      <div>
        {#if title}<svelte:element this={level === 3 ? 'h3' : 'h2'} class="title">{title}</svelte:element>{/if}
        {#if description}<p class="muted">{description}</p>{/if}
      </div>
      {#if actions}<div class="actions">{@render actions()}</div>{/if}
    </header>
  {/if}
  <div class:body={!flush}>
    {@render children()}
  </div>
  {/if}
</section>

<style>
  /* A collapsible panel's header: title and description on the left, the
     shared disclosure chevron (base.css) on the right. */
  summary {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    align-items: center;
    column-gap: var(--space-4);
    padding: var(--space-4) var(--space-5);
    border-radius: var(--radius-lg);
    font-weight: var(--weight-semibold);
    transition: background var(--duration-fast);
  }
  summary:hover { background: var(--sunken); }
  summary::after { grid-column: 2; grid-row: 1 / span 2; margin-right: var(--space-1); }
  summary .title { grid-column: 1; }
  .description { grid-column: 1; margin-top: var(--space-1); font-weight: var(--weight-regular); font-size: var(--text-sm); }
  details[open] > summary { border-bottom: 1px solid var(--line); border-radius: var(--radius-lg) var(--radius-lg) 0 0; }
  .panel {
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: var(--radius-lg);
    box-shadow: var(--shadow);
    min-width: 0;
  }
  header {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    flex-wrap: wrap;
    gap: var(--space-4);
    padding: var(--space-4) var(--space-5);
    border-bottom: 1px solid var(--line);
  }
  header > div {
    min-width: 0;
  }
  .title {
    font-size: var(--text-lg);
  }
  header p {
    font-size: var(--text-sm);
    margin-top: 3px;
  }
  .actions {
    display: flex;
    gap: var(--space-2);
    flex-wrap: wrap;
    justify-content: flex-end;
  }
  .body {
    padding: var(--space-5);
  }
  @media (max-width: 520px) {
    header,
    summary,
    .body {
      padding: var(--space-4);
    }
  }
</style>
