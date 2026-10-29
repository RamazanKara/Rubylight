<script lang="ts">
  import Button from '../Button.svelte';
  import Icon from '../Icon.svelte';
  import { MAX_FILES, gameFileError, gameFileRow, type GameFileRow } from './gamefiles';

  let { id, rows = $bindable() }: { id: string; rows: GameFileRow[] } = $props();

  function remove(row: GameFileRow) {
    const index = rows.indexOf(row);
    if (index >= 0) rows.splice(index, 1);
  }
</script>

<div class="files" aria-describedby="{id}-hint">
  <p class="hint" id="{id}-hint">
    Keys in the game’s own <code>.ini</code> files, set when a stream starts the game and put back when the session
    ends. A value can follow the stream: <code>{'{stream_fps}'}</code> is the stream’s frame rate,
    <code>{'{stream_fps-1}'}</code> one below it. The file must already have the key, in the game’s folder.
  </p>
  {#if rows.length}
    <div class="head muted" aria-hidden="true">
      <span>File</span><span>Section</span><span>Key</span><span>Value</span>
    </div>
  {/if}
  {#each rows as row, index (row.id)}
    {@const error = gameFileError(row, rows)}
    <div class="item">
      <input
        class="input mono"
        aria-label="File {index + 1}"
        aria-invalid={error ? 'true' : undefined}
        placeholder="OptiScaler.ini"
        autocomplete="off"
        spellcheck="false"
        bind:value={row.path}
      />
      <input
        class="input mono"
        aria-label="Section {index + 1}"
        placeholder="Framerate"
        autocomplete="off"
        spellcheck="false"
        bind:value={row.section}
      />
      <input
        class="input mono"
        aria-label="Key {index + 1}"
        aria-invalid={error ? 'true' : undefined}
        placeholder="FramerateLimit"
        autocomplete="off"
        spellcheck="false"
        bind:value={row.key}
      />
      <input
        class="input mono"
        aria-label="Value {index + 1}"
        aria-invalid={error ? 'true' : undefined}
        placeholder={'{stream_fps}'}
        autocomplete="off"
        spellcheck="false"
        bind:value={row.value}
      />
      <button type="button" class="remove" title="Remove" aria-label="Remove key {index + 1}" onclick={() => remove(row)}>
        <Icon name="x" size={15} />
      </button>
      {#if error}<p class="error">{error}</p>{/if}
    </div>
  {/each}
  <div>
    <Button size="sm" icon="plus" disabled={rows.length >= MAX_FILES} onclick={() => rows.push(gameFileRow())}>
      Add key
    </Button>
  </div>
</div>

<style>
  .files {
    display: grid;
    gap: var(--space-2);
    min-width: 0;
  }
  .hint {
    font-size: var(--text-xs);
    color: var(--muted);
    line-height: 1.45;
  }
  .hint code {
    color: var(--ink-2);
  }
  .head,
  .item {
    display: grid;
    grid-template-columns: minmax(0, 3fr) minmax(0, 2fr) minmax(0, 2fr) minmax(0, 2fr) 30px;
    gap: var(--space-2);
    align-items: center;
  }
  .head {
    font-size: var(--text-xs);
  }
  .error {
    grid-column: 1 / -1;
    font-size: var(--text-xs);
    color: var(--danger);
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
  @media (max-width: 640px) {
    .head {
      display: none;
    }
    .item {
      grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
    }
    .item > :first-child {
      grid-column: 1 / -1;
    }
    .remove {
      justify-self: end;
    }
  }
</style>
