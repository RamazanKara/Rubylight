<script lang="ts">
  import Button from '../Button.svelte';
  import Icon from '../Icon.svelte';
  import { api, type GameSettingsStatus } from '../../lib/api';
  import { textRow, type AppDraft } from './draft.svelte';
  import { settingsFileError, settingsFilePaths } from './settingsfiles';

  let { draft, id }: { draft: AppDraft; id: string } = $props();

  let status = $state<GameSettingsStatus | null>(null);
  let failure = $state('');

  /** The saved list: the host's copies follow it, not unsaved edits. */
  const savedKey = $derived(JSON.stringify(draft.saved['game-settings-files'] ?? []));

  $effect(() => {
    const uuid = draft.saved.uuid;
    void savedKey;
    status = null;
    failure = '';
    if (!uuid || !settingsFilePaths(draft.settingsFiles.map((row) => row.value)).length) return;
    let current = true;
    api.gameSettings
      .status(uuid)
      .then((value) => {
        if (current) status = value;
      })
      .catch((error: unknown) => {
        if (current) failure = error instanceof Error ? error.message : String(error);
      });
    return () => {
      current = false;
    };
  });

  function deviceState(saved: string[], files: string[]): string {
    if (!files.length) return '';
    if (saved.length === files.length) return 'Saved copy';
    if (!saved.length) return 'Not yet: its first launch keeps the current files';
    return `Saved copy of ${saved.length} of ${files.length} files`;
  }
</script>

<div class="settings-files">
  <fieldset class="list" aria-describedby="{id}-hint">
    <legend>Game settings files</legend>
    <p class="hint" id="{id}-hint">
      Each device gets its own copy of these files, such as the game’s resolution and quality. The PC’s own files
      come back when the game closes. Use the full path; %LOCALAPPDATA%, %APPDATA%, %USERPROFILE% and other Windows
      variables work.
    </p>
    {#each draft.settingsFiles as row, index (row.key)}
      {@const error = settingsFileError(row.value)}
      <div class="item">
        <input
          class="input mono"
          aria-label="Game settings file {index + 1}"
          aria-invalid={error ? 'true' : undefined}
          autocomplete="off"
          spellcheck="false"
          placeholder="%LOCALAPPDATA%\Game\Saved\Config\GameUserSettings.ini"
          bind:value={row.value}
        />
        <button
          type="button"
          class="remove"
          title="Remove"
          aria-label="Remove game settings file {index + 1}"
          onclick={() => draft.settingsFiles.splice(index, 1)}
        >
          <Icon name="x" size={15} />
        </button>
      </div>
      {#if error}<p class="error" role="alert">{error}</p>{/if}
    {/each}
    {#if draft.settingsFilesError && !draft.settingsFiles.some((row) => settingsFileError(row.value))}
      <p class="error" role="alert">{draft.settingsFilesError}</p>
    {/if}
    <div>
      <Button size="sm" icon="plus" onclick={() => draft.settingsFiles.push(textRow())}>Add settings file</Button>
    </div>
  </fieldset>

  {#if failure}
    <p class="error" role="alert">Could not read the saved copies: {failure}</p>
  {:else if status && status.files.length}
    {#each status.pending.filter((swap) => !swap.running && swap.error) as swap (swap.device)}
      <p class="error" role="alert">
        The files are not back from {swap.deviceName || swap.device} yet: {swap.error}
      </p>
    {/each}
    {#if status.devices.length}
      <ul class="devices" aria-label="Saved copies by device">
        {#each status.devices as device (device.uuid)}
          {@const running = status.pending.some((swap) => swap.device === device.uuid && swap.running)}
          <li>
            <span class="name">{device.name || device.uuid}</span>
            <span class="muted">{running ? 'In use now' : deviceState(device.saved, status.files)}</span>
          </li>
        {/each}
      </ul>
    {:else}
      <p class="hint">Pair a device to give it its own copy.</p>
    {/if}
  {/if}
</div>

<style>
  .settings-files {
    display: grid;
    gap: var(--space-4);
  }
  .list {
    display: grid;
    gap: var(--space-2);
    margin: 0;
    padding: 0;
    border: 0;
    min-width: 0;
  }
  legend {
    padding: 0;
    margin-bottom: var(--space-1);
    font-weight: var(--weight-medium);
    font-size: var(--text-sm);
  }
  .hint {
    font-size: var(--text-xs);
    color: var(--muted);
    line-height: 1.45;
  }
  .error {
    font-size: var(--text-xs);
    color: var(--danger);
    line-height: 1.45;
  }
  .item {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    gap: var(--space-2);
    align-items: center;
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
  .devices {
    display: grid;
    gap: var(--space-2);
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .devices li {
    display: flex;
    justify-content: space-between;
    gap: var(--space-3);
    font-size: var(--text-sm);
  }
  .name {
    font-weight: var(--weight-medium);
  }
</style>
