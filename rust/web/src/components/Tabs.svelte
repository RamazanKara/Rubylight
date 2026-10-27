<script lang="ts" module>
  export interface Tab {
    id: string;
    label: string;
    /** A count or short note after the label, such as unsaved changes. */
    badge?: string | number;
    disabled?: boolean;
  }
</script>

<script lang="ts">
  // In-page tabs (WAI-ARIA tabs pattern): one tab stop, arrows move and select,
  // Home and End jump. For links between pages use a nav with aria-current.
  import type { Snippet } from 'svelte';
  import { rovingIndex } from '../lib/design/keys';

  let {
    tabs,
    selected = $bindable(tabs[0]?.id),
    label,
    onchange,
    panel,
  }: {
    tabs: Tab[];
    selected?: string;
    /** Names the tab list for screen readers. */
    label: string;
    onchange?: (id: string) => void;
    /** Renders the selected tab's content. */
    panel: Snippet<[string]>;
  } = $props();

  const base = $props.id();
  const buttons: HTMLButtonElement[] = $state([]);
  const index = $derived(Math.max(0, tabs.findIndex((tab) => tab.id === selected)));

  function select(id: string) {
    if (id === selected) return;
    selected = id;
    onchange?.(id);
  }

  function keydown(event: KeyboardEvent) {
    const next = rovingIndex(index, event.key, tabs.length, 'horizontal', (i) => !!tabs[i]?.disabled);
    const target = next === null ? undefined : tabs[next];
    if (next === null || !target) return;
    event.preventDefault();
    select(target.id);
    buttons[next]?.focus();
  }
</script>

<div class="tabs">
  <div class="list" role="tablist" aria-label={label} tabindex="-1" onkeydown={keydown}>
    {#each tabs as tab, i (tab.id)}
      <button
        bind:this={buttons[i]}
        type="button"
        role="tab"
        id="{base}-tab-{tab.id}"
        aria-controls="{base}-panel-{tab.id}"
        aria-selected={i === index}
        tabindex={i === index ? 0 : -1}
        disabled={tab.disabled}
        onclick={() => select(tab.id)}
      >
        {tab.label}
        {#if tab.badge !== undefined && tab.badge !== ''}<span class="count">{tab.badge}</span>{/if}
      </button>
    {/each}
  </div>
  {#if tabs[index]}
    {@const tab = tabs[index]}
    <div class="panel" role="tabpanel" id="{base}-panel-{tab.id}" aria-labelledby="{base}-tab-{tab.id}" tabindex="0">
      {@render panel(tab.id)}
    </div>
  {/if}
</div>

<style>
  .tabs {
    display: grid;
    gap: var(--space-4);
    min-width: 0;
  }
  .list {
    display: flex;
    gap: var(--space-1);
    overflow-x: auto;
    border-bottom: 1px solid var(--line);
    scrollbar-width: thin;
  }
  .list:focus {
    outline: none;
  }
  [role='tab'] {
    position: relative;
    display: inline-flex;
    align-items: center;
    gap: var(--space-2);
    flex: none;
    padding: var(--space-2) var(--space-3);
    border: 0;
    border-radius: var(--radius) var(--radius) 0 0;
    background: none;
    color: var(--ink-2);
    font-size: var(--text-sm);
    font-weight: var(--weight-medium);
    cursor: pointer;
    transition: color var(--duration-fast) var(--ease-standard), background var(--duration-fast) var(--ease-standard);
  }
  [role='tab']::after {
    content: '';
    position: absolute;
    inset: auto var(--space-2) -1px;
    height: 3px;
    border-radius: var(--radius-pill);
    background: var(--accent);
    transform: scaleX(0);
    transition: transform var(--duration-base) var(--ease-standard);
  }
  [role='tab']:hover:not(:disabled) {
    background: var(--sunken);
    color: var(--ink);
  }
  [role='tab'][aria-selected='true'] {
    color: var(--ink);
    font-weight: var(--weight-semibold);
  }
  [role='tab'][aria-selected='true']::after {
    transform: scaleX(1);
  }
  [role='tab']:focus-visible {
    outline-offset: calc(var(--focus-width) * -1);
  }
  [role='tab']:disabled {
    color: var(--muted);
    cursor: default;
  }
  .count {
    padding: 0 var(--space-2);
    border-radius: var(--radius-pill);
    background: var(--accent-soft);
    color: var(--ink);
    font-size: var(--text-xs);
    line-height: var(--leading);
  }
  .panel {
    min-width: 0;
  }
  .panel:focus-visible {
    outline-offset: var(--focus-offset);
  }
</style>
