<script lang="ts">
  // A message about a page or a section: a state to know about, a problem
  // with its fix, or a result. Toasts are for passing confirmations instead.
  import type { Snippet } from 'svelte';
  import Icon, { type IconName } from './Icon.svelte';

  let {
    tone = 'info',
    title,
    live = false,
    ondismiss,
    children,
    actions,
  }: {
    tone?: 'info' | 'ok' | 'warn' | 'danger' | 'accent';
    title?: string;
    /** Announce it when it appears (alert for danger, status otherwise). */
    live?: boolean;
    ondismiss?: () => void;
    children?: Snippet;
    /** The fix: one or two buttons or links. */
    actions?: Snippet;
  } = $props();

  const icons: Record<typeof tone, IconName> = { info: 'info', ok: 'check', warn: 'alert', danger: 'alert', accent: 'info' };
  const role = $derived(live ? (tone === 'danger' ? 'alert' : 'status') : undefined);
</script>

<div class="banner {tone}" {role}>
  <span class="icon"><Icon name={icons[tone]} size={20} /></span>
  <div class="text">
    {#if title}<p class="title">{title}</p>{/if}
    {#if children}<div class="body">{@render children()}</div>{/if}
    {#if actions}<div class="actions">{@render actions()}</div>{/if}
  </div>
  {#if ondismiss}
    <button type="button" class="dismiss" aria-label="Dismiss" onclick={ondismiss}><Icon name="x" size={16} /></button>
  {/if}
</div>

<style>
  .banner {
    display: flex;
    gap: var(--space-3);
    align-items: flex-start;
    padding: var(--space-3) var(--space-4);
    border: 1px solid var(--line);
    border-radius: var(--radius);
    background: var(--sunken);
    color: var(--ink);
    font-size: var(--text-sm);
    line-height: var(--leading-snug);
  }
  .ok {
    background: var(--ok-soft);
    border-color: transparent;
  }
  .warn {
    background: var(--warn-soft);
    border-color: transparent;
  }
  .danger {
    background: var(--danger-soft);
    border-color: transparent;
  }
  .accent {
    background: var(--accent-soft);
    border-color: transparent;
  }
  .icon {
    display: grid;
    place-items: center;
    height: calc(var(--text-sm) * 1.4);
    color: var(--muted);
  }
  .ok .icon {
    color: var(--ok);
  }
  .warn .icon {
    color: var(--warn);
  }
  .danger .icon {
    color: var(--danger);
  }
  .accent .icon {
    color: var(--accent-text);
  }
  .text {
    flex: 1;
    min-width: 0;
    display: grid;
    gap: var(--space-1);
  }
  .title {
    font-weight: var(--weight-semibold);
  }
  .body {
    color: var(--ink-2);
  }
  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-2);
    margin-top: var(--space-2);
  }
  .dismiss {
    display: grid;
    place-items: center;
    margin: calc(var(--space-3) * -1) calc(var(--space-3) * -1) calc(var(--space-3) * -1) 0;
    border: 0;
    border-radius: var(--radius);
    background: none;
    color: var(--ink-2);
    cursor: pointer;
  }
  .dismiss:hover {
    color: var(--ink);
    background: color-mix(in srgb, var(--ink) 8%, transparent);
  }
</style>
