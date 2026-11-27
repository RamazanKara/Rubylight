<script lang="ts">
  import { tick } from 'svelte';
  import Button from '../Button.svelte';
  import Icon from '../Icon.svelte';
  import PathPicker from '../PathPicker.svelte';
  import type { ConfigValue } from '../../lib/api';
  import { folderErrors, hasFolderErrors, newFolder, readFolders, type SharedFolder } from '../../lib/shared-folders';

  let {
    id,
    value,
    onchange,
    oninvalid,
  }: {
    id: string;
    value: ConfigValue;
    onchange: (value: ConfigValue) => void;
    oninvalid?: (text: string | null) => void;
  } = $props();

  const folderList = $derived(readFolders(value));
  const folders = $derived(folderList.folders);
  const folderProblems = $derived(folderErrors(folders));

  let rowsElement = $state<HTMLElement>();

  async function addRow(next: SharedFolder[]) {
    onchange(next);
    await tick();
    const rows = rowsElement?.querySelectorAll('[data-row]');
    rows?.[rows.length - 1]?.querySelector('input')?.focus();
  }

  const without = <T,>(list: T[], index: number) => list.filter((_, at) => at !== index);

  // The rows are kept while they have errors, so each one shows where it is
  // wrong; the draft also marks the setting invalid, which holds the save.
  function setFolders(next: SharedFolder[]) {
    onchange(next);
    oninvalid?.(hasFolderErrors(next) ? JSON.stringify(next) : null);
  }
  function setFolder(index: number, change: Partial<SharedFolder>) {
    setFolders(folders.map((folder, at) => (at === index ? { ...folder, ...change } : folder)));
  }
</script>

<div class="rows" {id} bind:this={rowsElement}>
  {#if folderList.problem}
    <p class="error" id="{id}-unreadable">
      The saved list can’t be read, so no folder is shared: {folderList.problem}. Adding a folder here replaces it.
    </p>
  {/if}
  {#each folders as folder, index}
    {@const problem = folderProblems[index] ?? { name: '', path: '', id: '' }}
    <div class="command" data-row>
      <div class="cell">
        <label class="cell-label" for="{id}-{index}-name">Name<span class="visually-hidden">, folder {index + 1}</span></label>
        <input
          id="{id}-{index}-name"
          class="input"
          autocomplete="off"
          aria-invalid={problem.name ? 'true' : undefined}
          aria-describedby={problem.name ? `${id}-${index}-name-error` : undefined}
          value={folder.name}
          oninput={(event) => setFolder(index, { name: event.currentTarget.value })}
        />
        {#if problem.name}<p class="error cell-error" id="{id}-{index}-name-error">{problem.name}</p>{/if}
      </div>
      <div class="cell">
        <label class="cell-label" for="{id}-{index}-path">Path<span class="visually-hidden">, folder {index + 1}</span></label>
        <PathPicker kind="directory" value={folder.path} onpick={(path) => setFolder(index, { path })}>
          <input
            id="{id}-{index}-path"
            class="input mono"
            autocomplete="off"
            spellcheck="false"
            placeholder="D:\Games"
            aria-invalid={problem.path ? 'true' : undefined}
            aria-describedby={problem.path ? `${id}-${index}-path-error` : undefined}
            value={folder.path}
            oninput={(event) => setFolder(index, { path: event.currentTarget.value })}
          />
        </PathPicker>
        {#if problem.path}<p class="error cell-error" id="{id}-{index}-path-error">{problem.path}</p>{/if}
      </div>
      <div class="command-foot">
        {#if problem.id}
          <p class="error">{problem.id}</p>
        {:else}
          <span class="note">{folder.id ? `Id: ${folder.id}` : 'Gets an id from its name when saved'}</span>
        {/if}
        <button
          type="button"
          class="remove"
          aria-label="Remove folder {index + 1}"
          title="Remove"
          onclick={() => setFolders(without(folders, index))}
        >
          <Icon name="x" size={15} />
        </button>
      </div>
    </div>
  {:else}
    <p class="none">No folders are shared.</p>
  {/each}
  <div>
    <Button size="sm" icon="plus" onclick={() => addRow([...folders, newFolder()])}>Add folder</Button>
  </div>
</div>

<style>
  .note,
  .error,
  .none {
    font-size: var(--text-xs);
    color: var(--muted);
    line-height: 1.45;
  }
  .error {
    color: var(--danger);
  }
  [aria-invalid='true'] {
    border-color: var(--danger);
  }
  .rows {
    display: grid;
    gap: var(--space-2);
    min-width: 0;
  }
  .command {
    display: grid;
    gap: var(--space-1-5);
    padding: var(--space-2) var(--space-3);
    border: 1px solid var(--line);
    border-radius: var(--radius);
    background: var(--sunken);
    min-width: 0;
  }
  .cell {
    display: grid;
    grid-template-columns: 76px minmax(0, 1fr);
    align-items: center;
    gap: var(--space-2);
  }
  .cell-error {
    grid-column: 2;
  }
  .cell-label {
    font-size: var(--text-xs);
    font-weight: var(--weight-medium);
    color: var(--ink-2);
  }
  .command-foot {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-2);
  }
  .remove {
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
  @media (max-width: 520px) {
    .cell {
      grid-template-columns: minmax(0, 1fr);
      gap: var(--space-1);
    }
    .cell-error {
      grid-column: 1;
    }
  }
</style>
