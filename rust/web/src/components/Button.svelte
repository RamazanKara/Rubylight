<script lang="ts">
  import type { Snippet } from 'svelte';
  import { link } from '../lib/router.svelte';
  import Icon, { type IconName } from './Icon.svelte';

  let {
    variant = 'secondary',
    size = 'md',
    type = 'button',
    icon,
    href,
    disabled = false,
    busy = false,
    title,
    onclick,
    children,
  }: {
    variant?: 'primary' | 'secondary' | 'danger' | 'ghost';
    size?: 'sm' | 'md';
    type?: 'button' | 'submit';
    icon?: IconName;
    href?: string;
    disabled?: boolean;
    busy?: boolean;
    title?: string;
    onclick?: (event: MouseEvent) => void;
    children?: Snippet;
  } = $props();
</script>

{#if href}
  <a class="button {variant} {size}" {href} {title} use:link download={href.startsWith('/api/') ? '' : undefined}>
    {#if icon}<Icon name={icon} size={size === 'sm' ? 15 : 17} />{/if}
    {@render children?.()}
  </a>
{:else}
  <button class="button {variant} {size}" {type} {title} disabled={disabled || busy} aria-busy={busy} {onclick}>
    {#if busy}<span class="spinner" aria-hidden="true"></span>{:else if icon}<Icon
        name={icon}
        size={size === 'sm' ? 15 : 17}
      />{/if}
    {@render children?.()}
  </button>
{/if}

<style>
  .button {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 7px;
    border: 1px solid var(--line-strong);
    border-radius: var(--radius);
    background: var(--panel);
    color: var(--ink);
    font-weight: var(--weight-medium);
    font-size: var(--text-sm);
    line-height: 1.3;
    padding: 11px 18px;
    min-height: var(--control-height);
    cursor: pointer;
    text-decoration: none;
    max-width: 100%;
    white-space: normal;
    /* Wrap between words only: the page-wide "anywhere" would let a squeezed
       button break its label mid-word ("Brow se"). */
    overflow-wrap: break-word;
    text-align: center;
    transition:
      background var(--duration-fast) var(--ease-standard),
      border-color var(--duration-fast) var(--ease-standard),
      color var(--duration-fast) var(--ease-standard),
      transform var(--duration-fast) var(--ease-standard);
  }
  /* A press nudges the button down a pixel: feedback without a layout shift. */
  .button:active:not(:disabled) {
    transform: translateY(1px);
  }
  .sm {
    padding: 9px 13px;
  }
  .button:hover:not(:disabled) {
    background: var(--sunken);
    border-color: var(--muted);
  }
  .primary {
    background: var(--accent);
    border-color: var(--accent);
    color: var(--accent-ink);
  }
  .primary:hover:not(:disabled) {
    background: var(--accent-strong);
    border-color: var(--accent-strong);
  }
  .danger {
    color: var(--danger);
  }
  .danger:hover:not(:disabled) {
    background: var(--danger-soft);
    border-color: var(--danger);
  }
  .ghost {
    border-color: transparent;
    background: transparent;
    color: var(--ink-2);
  }
  .ghost:hover:not(:disabled) {
    background: var(--sunken);
    border-color: transparent;
  }
  .button:disabled {
    opacity: 0.55;
    cursor: default;
  }
  .button :global(svg),
  .spinner {
    flex: none;
  }
  .spinner {
    width: 13px;
    height: 13px;
    border: 2px solid currentColor;
    border-right-color: transparent;
    border-radius: 50%;
    animation: spin 0.7s linear infinite;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
</style>
