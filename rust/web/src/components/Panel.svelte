<script lang="ts">
  import type { Snippet } from 'svelte';
  import Disclosure from './Disclosure.svelte';

  let {
    title,
    description,
    actions,
    flush = false,
    collapsible = false,
    open = false,
    level = 2,
    id,
    children,
  }: {
    title?: string;
    description?: string;
    actions?: Snippet;
    /** Content reaches the panel's edges (tables, lists). */
    flush?: boolean;
    /** A folding card: the console's one disclosure, its title the same heading as an open panel's. */
    collapsible?: boolean;
    open?: boolean;
    /** Heading level of the title: 3 inside a titled page section. It looks the same. */
    level?: 2 | 3;
    /** An id for links into the page, on the panel itself. */
    id?: string;
    children: Snippet;
  } = $props();
</script>

{#if collapsible}
  <Disclosure title={title ?? ''} {description} {open} {level} {flush} {id}>{@render children()}</Disclosure>
{:else}
  <section class="panel" {id}>
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
  </section>
{/if}

<style>
  .panel {
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: var(--radius-lg);
    box-shadow: var(--shadow);
    min-width: 0;
    scroll-margin-top: var(--sticky-offset, var(--space-4));
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
  /* A card title: one step below the page's section headings (Section.svelte). */
  .title {
    font-size: var(--text-md);
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
    .body {
      padding: var(--space-4);
    }
    /* Actions that wrap under the title stay close to it. */
    header {
      gap: var(--space-2) var(--space-4);
    }
  }
</style>
