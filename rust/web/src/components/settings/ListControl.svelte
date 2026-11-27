<script lang="ts">
  import { tick } from 'svelte';
  import Button from '../Button.svelte';
  import Icon from '../Icon.svelte';
  import Switch from './Switch.svelte';
  import type { ConfigValue, PrepCommand } from '../../lib/api';
  import type { Setting } from '../../lib/settings-types';
  import { toCommands, toList, toServerCommands, type ServerCommand } from './values';

  let {
    setting,
    id,
    value,
    onchange,
    describedby,
  }: {
    setting: Setting;
    id: string;
    value: ConfigValue;
    onchange: (value: ConfigValue) => void;
    describedby?: string;
  } = $props();

  const control = $derived(setting.control);
  const label = $derived(setting.label);

  const items = $derived(control.kind === 'list' ? toList(value) : []);
  const commands = $derived(control.kind === 'commands' ? toCommands(value) : []);
  const serverCommands = $derived(control.kind === 'server-commands' ? toServerCommands(value) : []);

  let rowsElement = $state<HTMLElement>();

  async function addRow(next: unknown[]) {
    onchange(next);
    await tick();
    const rows = rowsElement?.querySelectorAll('[data-row]');
    rows?.[rows.length - 1]?.querySelector('input')?.focus();
  }

  const without = <T,>(list: T[], index: number) => list.filter((_, at) => at !== index);

  function setItem(index: number, text: string) {
    onchange(items.map((item, at) => (at === index ? text : item)));
  }
  function setCommand(index: number, change: Partial<PrepCommand>) {
    onchange(commands.map((row, at) => (at === index ? { ...row, ...change } : row)));
  }
  function setServerCommand(index: number, change: Partial<ServerCommand>) {
    onchange(serverCommands.map((row, at) => (at === index ? { ...row, ...change } : row)));
  }
</script>

{#if control.kind === 'list'}
  <div class="rows" {id} bind:this={rowsElement}>
    {#each items as item, index}
      <div class="item" data-row>
        <input
          class="input mono"
          aria-label="{label}, item {index + 1}"
          autocomplete="off"
          spellcheck="false"
          placeholder={control.placeholder}
          value={item}
          oninput={(event) => setItem(index, event.currentTarget.value)}
        />
        <button
          type="button"
          class="remove"
          aria-label="Remove item {index + 1}"
          title="Remove"
          onclick={() => onchange(without(items, index))}
        >
          <Icon name="x" size={15} />
        </button>
      </div>
    {:else}
      <p class="none">No entries.</p>
    {/each}
    <div><Button size="sm" icon="plus" onclick={() => addRow([...items, ''])}>Add</Button></div>
  </div>
{:else if control.kind === 'commands'}
  <div class="rows" {id} bind:this={rowsElement}>
    {#each commands as row, index}
      <div class="command" data-row>
        <label class="cell">
          <span class="cell-label">{setting.key === 'global_state_cmd' ? 'On resume' : 'Before launch'}<span class="visually-hidden">, command {index + 1}</span></span>
          <input
            class="input mono"
            autocomplete="off"
            spellcheck="false"
            value={row.do}
            oninput={(event) => setCommand(index, { do: event.currentTarget.value })}
          />
        </label>
        <label class="cell">
          <span class="cell-label">{setting.key === 'global_state_cmd' ? 'On pause' : 'After exit'}<span class="visually-hidden">, command {index + 1}</span></span>
          <input
            class="input mono"
            autocomplete="off"
            spellcheck="false"
            value={row.undo}
            oninput={(event) => setCommand(index, { undo: event.currentTarget.value })}
          />
        </label>
        <div class="command-foot">
          <label class="check">
            <input
              type="checkbox"
              checked={row.elevated}
              onchange={(event) => setCommand(index, { elevated: event.currentTarget.checked })}
            />
            As administrator
          </label>
          <button
            type="button"
            class="remove"
            aria-label="Remove command {index + 1}"
            title="Remove"
            onclick={() => onchange(without(commands, index))}
          >
            <Icon name="x" size={15} />
          </button>
        </div>
      </div>
    {:else}
      <p class="none">No commands.</p>
    {/each}
    <div>
      <Button size="sm" icon="plus" onclick={() => addRow([...commands, { do: '', undo: '', elevated: false }])}>
        Add command
      </Button>
    </div>
  </div>
{:else if control.kind === 'server-commands'}
  <div class="rows" {id} bind:this={rowsElement}>
    {#each serverCommands as row, index}
      <div class="command" data-row>
        <label class="cell">
          <span class="cell-label">Name<span class="visually-hidden">, command {index + 1}</span></span>
          <input
            class="input"
            autocomplete="off"
            value={row.name}
            oninput={(event) => setServerCommand(index, { name: event.currentTarget.value })}
          />
        </label>
        <label class="cell">
          <span class="cell-label">Command<span class="visually-hidden">{` ${index + 1}`}</span></span>
          <input
            class="input mono"
            autocomplete="off"
            spellcheck="false"
            value={row.cmd}
            oninput={(event) => setServerCommand(index, { cmd: event.currentTarget.value })}
          />
        </label>
        <div class="command-foot">
          <label class="check">
            <input
              type="checkbox"
              checked={row.elevated}
              onchange={(event) => setServerCommand(index, { elevated: event.currentTarget.checked })}
            />
            As administrator
          </label>
          <button
            type="button"
            class="remove"
            aria-label="Remove command {index + 1}"
            title="Remove"
            onclick={() => onchange(without(serverCommands, index))}
          >
            <Icon name="x" size={15} />
          </button>
        </div>
      </div>
    {:else}
      <p class="none">No PC commands.</p>
    {/each}
    <div>
      <Button size="sm" icon="plus" onclick={() => addRow([...serverCommands, { name: '', cmd: '', elevated: false }])}>
        Add command
      </Button>
    </div>
  </div>
{:else if control.kind === 'flags'}
  <div class="rows" {id} role="group" aria-label={label}>
    {#each control.options as option (option.value)}
      {@const on = toList(value).includes(option.value)}
      <div class="item">
        <span>{option.label}</span>
        <Switch
          id="{id}-{option.value}"
          {describedby}
          checked={on}
          onchange={(checked) => {
            const rest = toList(value).filter((item) => item !== option.value);
            onchange(
              (control.kind === 'flags' ? control.options : [])
                .map((candidate) => candidate.value)
                .filter((name) => (name === option.value ? checked : rest.includes(name))),
            );
          }}
        />
      </div>
    {/each}
  </div>
{/if}

<style>
  .none {
    font-size: var(--text-xs);
    color: var(--muted);
    line-height: 1.45;
  }
  .rows {
    display: grid;
    gap: var(--space-2);
    min-width: 0;
  }
  .item {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    align-items: center;
    gap: var(--space-2);
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
  .check {
    display: inline-flex;
    align-items: center;
    gap: var(--space-1-5);
    font-size: var(--text-sm);
    cursor: pointer;
  }
  .check input {
    margin: 0;
    accent-color: var(--ink-2);
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
  }
</style>
