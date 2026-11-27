<script lang="ts">
  import Icon from '../Icon.svelte';
  import type { SettingsTab, TabId } from '../../lib/settings-schema';

  let {
    query = $bindable(''),
    tabs,
    activeId,
    browsingResults,
    changedIn,
    onopen,
    onsearchinput,
  }: {
    query: string;
    tabs: readonly SettingsTab[];
    activeId: TabId;
    browsingResults: boolean;
    changedIn: Partial<Record<TabId, number>>;
    onopen: (id: string, event?: MouseEvent) => void;
    onsearchinput: () => void;
  } = $props();
</script>

<div class="side">
  <div class="search">
    <Icon name="search" size={16} />
    <input
      class="input"
      type="search"
      placeholder="Search settings"
      aria-label="Search settings"
      autocomplete="off"
      spellcheck="false"
      bind:value={query}
      oninput={onsearchinput}
    />
  </div>

  <label class="picker">
    <span>Settings tab</span>
    <select aria-label="Settings tab" class="select" onchange={(event) => onopen(event.currentTarget.value)}>
      {#if browsingResults}<option value="" disabled selected>All tabs</option>{/if}
      {#each tabs as tab (tab.id)}
        {@const count = changedIn[tab.id] ?? 0}
        <option value={tab.id} selected={!browsingResults && tab.id === activeId}>
          {tab.label}{count ? ` (${count} unsaved)` : ''}
        </option>
      {/each}
    </select>
  </label>

  <nav class="tabs" aria-label="Settings tabs">
    <ul>
      {#each tabs as tab (tab.id)}
        {@const count = changedIn[tab.id] ?? 0}
        <li>
          <a
            href="/settings/{tab.id}"
            aria-current={!browsingResults && tab.id === activeId ? 'page' : undefined}
            onclick={(event) => onopen(tab.id, event)}
          >
            <span class="name">
              {tab.label}
              {#if count}
                <span class="dot" aria-hidden="true"></span>
                <span class="visually-hidden">, {count} unsaved</span>
              {/if}
            </span>
          </a>
        </li>
      {/each}
    </ul>
  </nav>
</div>

<style>
  .side {
    display: grid;
    gap: var(--space-3);
    min-width: 0;
  }
  /* As wide as the search on the Settings home, so the two pages line up. */
  .search {
    max-width: 640px;
    position: relative;
    color: var(--muted);
  }
  .search :global(svg) {
    position: absolute;
    left: 10px;
    top: 50%;
    transform: translateY(-50%);
    pointer-events: none;
  }
  .search .input {
    padding-left: var(--space-6);
  }
  .picker {
    display: none;
    gap: var(--space-1-5);
    font-size: var(--text-sm);
    font-weight: var(--weight-medium);
  }
  .tabs ul {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-1);
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .tabs a {
    display: grid;
    gap: 1px;
    padding: var(--space-3) calc(var(--space-4) - 2px);
    min-height: var(--control-height);
    border-radius: var(--radius);
    color: var(--ink-2);
    text-decoration: none;
  }
  .tabs a:hover {
    background: var(--panel);
    color: var(--ink);
  }
  .tabs a[aria-current='page'] {
    background: var(--panel);
    color: var(--ink);
    box-shadow: var(--mark-tab);
  }
  .name {
    display: flex;
    align-items: center;
    gap: var(--space-1-5);
    font-weight: var(--weight-medium);
    font-size: var(--text-sm);
  }
  .dot {
    flex: none;
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--accent);
  }
  @container (max-width: 720px) {
    .side {
      position: static;
      grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
      align-items: end;
    }
    .picker {
      display: grid;
    }
    .tabs {
      display: none;
    }
  }
  @media (max-width: 520px) {
    .side {
      grid-template-columns: minmax(0, 1fr);
    }
  }
</style>
