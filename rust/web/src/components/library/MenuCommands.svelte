<script lang="ts">
  import { tick } from 'svelte';
  import Button from '../Button.svelte';
  import Icon from '../Icon.svelte';
  import { MENU_LIMIT, menuRow, type MenuRow } from './menucommands';

  let { rows = $bindable(), id, error = '' }: { rows: MenuRow[]; id: string; error?: string } = $props();

  let list = $state<HTMLElement>();

  async function add() {
    const row = menuRow();
    rows.push(row);
    await tick();
    list?.querySelector<HTMLElement>(`[data-key="${row.key}"] input`)?.focus();
  }
</script>

<div class="menu" bind:this={list}>
  {#each rows as row, index (row.key)}
    <fieldset class="row-item" data-key={row.key}>
      <legend class="legend">Command {index + 1}</legend>
      <label class="cell">
        <span class="cell-label">Name in the stream menu</span>
        <input id="{id}-name-{row.key}" class="input" autocomplete="off" placeholder="Desktop pet" bind:value={row.name} />
      </label>
      <label class="cell">
        <span class="cell-label">Command</span>
        <input id="{id}-cmd-{row.key}" class="input mono" autocomplete="off" spellcheck="false" bind:value={row.cmd} />
      </label>
      <label class="cell">
        <span class="cell-label">Undo when the app closes</span>
        <input
          id="{id}-undo-{row.key}"
          class="input mono"
          autocomplete="off"
          spellcheck="false"
          placeholder="Optional"
          bind:value={row.undo}
        />
      </label>
      <div class="controls">
        <label class="check">
          <input type="checkbox" bind:checked={row.elevated} />
          As administrator
        </label>
        <button
          type="button"
          class="icon-button remove"
          title="Remove"
          aria-label="Remove menu command {index + 1}"
          onclick={() => rows.splice(index, 1)}
        >
          <Icon name="x" size={15} />
        </button>
      </div>
    </fieldset>
  {:else}
    <p class="muted none">No menu commands.</p>
  {/each}
  {#if error}
    <p class="error" role="alert">{error}</p>
  {/if}
  <div>
    <Button size="sm" icon="plus" disabled={rows.length >= MENU_LIMIT} onclick={add}>Add menu command</Button>
  </div>
</div>

<style>
  .menu {
    display: grid;
    gap: var(--space-2);
    min-width: 0;
  }
  .row-item {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(180px, 1fr));
    gap: var(--space-2) var(--space-3);
    align-items: end;
    margin: 0;
    padding: 0 0 var(--space-3);
    border: 0;
    border-bottom: 1px solid var(--line);
    min-width: 0;
  }
  .legend {
    padding: 0;
    font-size: var(--text-xs);
    font-weight: var(--weight-semibold);
    color: var(--muted);
  }
  .cell {
    display: grid;
    gap: var(--space-1);
    min-width: 0;
  }
  .cell-label {
    font-size: var(--text-xs);
    color: var(--muted);
  }
  .controls {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-3);
  }
  .check {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: var(--text-sm);
    white-space: nowrap;
    cursor: pointer;
  }
  .check input {
    margin: 0;
    accent-color: var(--ink-2);
  }
  .icon-button {
    display: grid;
    place-items: center;
    width: 30px;
    height: 30px;
    border: 1px solid transparent;
    border-radius: var(--radius);
    background: none;
    color: var(--muted);
    cursor: pointer;
  }
  .remove:hover {
    background: var(--danger-soft);
    color: var(--danger);
  }
  .none {
    font-size: var(--text-sm);
  }
  .error {
    margin: 0;
    font-size: var(--text-sm);
    color: var(--danger);
  }
</style>
