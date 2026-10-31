<script lang="ts">
  import type { Snippet } from 'svelte';
  import Icon, { type IconName } from './Icon.svelte';

  let { icon, title, children }: { icon: IconName; title: string; children?: Snippet } = $props();
</script>

<div class="empty">
  <span class="icon"><Icon name={icon} size={28} /></span>
  <h3>{title}</h3>
  {#if children}<div class="body">{@render children()}</div>{/if}
</div>

<style>
  .empty {
    display: grid;
    justify-items: center;
    text-align: center;
    gap: var(--space-2);
    padding: var(--space-6) var(--space-5);
    color: var(--muted);
  }
  /* The icon sits in a soft well sized to the block, a step above the title. */
  .icon {
    display: grid;
    place-items: center;
    width: 56px;
    height: 56px;
    margin-bottom: var(--space-2);
    border-radius: 50%;
    background: var(--sunken);
    border: 1px solid var(--line);
    color: var(--ink-2);
  }
  h3 {
    color: var(--ink);
    font-size: var(--text-md);
  }
  .body {
    max-width: 52ch;
    font-size: var(--text-sm);
    display: grid;
    gap: var(--space-3);
    justify-items: center;
  }
</style>
