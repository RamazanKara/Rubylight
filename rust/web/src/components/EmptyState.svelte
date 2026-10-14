<script lang="ts">
  import type { Snippet } from 'svelte';
  import Icon, { type IconName } from './Icon.svelte';

  let { icon, title, children }: { icon: IconName; title: string; children?: Snippet } = $props();
</script>

<div class="empty">
  <Icon name={icon} size={24} />
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
  h3 {
    color: var(--ink);
  }
  .body {
    max-width: 52ch;
    font-size: var(--text-sm);
    display: grid;
    gap: var(--space-3);
    justify-items: center;
  }
</style>
