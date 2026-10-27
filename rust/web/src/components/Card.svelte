<script lang="ts">
  // A surface for one item in a collection (a device, an app, a check).
  // Panel is the titled section of a page; Card is what repeats inside one.
  import type { Snippet } from 'svelte';
  import { link } from '../lib/router.svelte';

  let {
    href,
    label,
    selected = false,
    padding = 'md',
    media,
    children,
    footer,
  }: {
    /** Makes the whole card one link; keep other controls out of it. */
    href?: string;
    /** Accessible name of a linked card, when its text alone is unclear. */
    label?: string;
    selected?: boolean;
    padding?: 'none' | 'sm' | 'md';
    /** Art or a preview above the content, edge to edge. */
    media?: Snippet;
    children: Snippet;
    /** Actions or meta below a divider. */
    footer?: Snippet;
  } = $props();
</script>

{#snippet content()}
  {#if media}<div class="media">{@render media()}</div>{/if}
  <div class="body {padding}">{@render children()}</div>
  {#if footer}<div class="footer {padding}">{@render footer()}</div>{/if}
{/snippet}

{#if href}
  <a class="card interactive" class:selected {href} aria-label={label} aria-current={selected ? 'true' : undefined} use:link>
    {@render content()}
  </a>
{:else}
  <div class="card" class:selected>
    {@render content()}
  </div>
{/if}

<style>
  .card {
    display: flex;
    flex-direction: column;
    min-width: 0;
    overflow: hidden;
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: var(--radius-lg);
    box-shadow: var(--shadow);
    color: inherit;
    text-decoration: none;
  }
  .interactive {
    transition:
      border-color var(--duration-fast) var(--ease-standard),
      box-shadow var(--duration-fast) var(--ease-standard),
      transform var(--duration-fast) var(--ease-standard);
  }
  .interactive:hover {
    border-color: var(--line-strong);
    box-shadow: var(--shadow-raised);
    transform: translateY(-1px);
  }
  .interactive:active {
    transform: none;
  }
  .selected {
    border-color: var(--accent);
    box-shadow: 0 0 0 1px var(--accent);
  }
  .media {
    background: var(--media-bg);
  }
  .media :global(img) {
    display: block;
    width: 100%;
    height: auto;
  }
  .body {
    flex: 1;
    display: grid;
    gap: var(--space-2);
    align-content: start;
  }
  .footer {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-2);
    border-top: 1px solid var(--line);
  }
  .md {
    padding: var(--space-4) var(--space-5);
  }
  .sm {
    padding: var(--space-3) var(--space-4);
  }
  .none {
    padding: 0;
  }
</style>
