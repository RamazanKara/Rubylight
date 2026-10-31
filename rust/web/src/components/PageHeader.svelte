<script lang="ts">
  import type { Snippet } from 'svelte';

  let { title, subtitle, actions }: { title: string; subtitle?: string; actions?: Snippet } = $props();
</script>

<!-- One rule on every page: actions sit on the title's line, centred on it, and
     the subtitle runs underneath. On a phone they move under the subtitle. -->
<header class="page-header" class:has-subtitle={subtitle}>
  <h1>{title}</h1>
  {#if subtitle}<p class="subtitle muted">{subtitle}</p>{/if}
  {#if actions}<div class="actions">{@render actions()}</div>{/if}
</header>

<style>
  .page-header {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    grid-template-areas:
      'title actions'
      'subtitle subtitle';
    align-items: center;
    gap: var(--space-2) var(--space-4);
  }
  h1 {
    grid-area: title;
    min-width: 0;
  }
  .subtitle {
    grid-area: subtitle;
    max-width: 70ch;
  }
  .actions {
    grid-area: actions;
    display: flex;
    justify-content: flex-end;
    align-items: center;
    gap: var(--space-2);
    flex-wrap: wrap;
  }
  @media (max-width: 600px) {
    .page-header.has-subtitle {
      grid-template-columns: minmax(0, 1fr);
      grid-template-areas:
        'title'
        'subtitle'
        'actions';
    }
    .has-subtitle .actions {
      justify-content: flex-start;
      margin-top: var(--space-2);
    }
  }
</style>
