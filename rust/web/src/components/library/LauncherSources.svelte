<script lang="ts" module>
  import type { LibraryDiscovery, LibrarySource } from '../../lib/api';

  /** The setting that turns each launcher on, the same switch for every launcher. */
  export const LAUNCHER_KEYS: Record<LibrarySource, string | null> = {
    steam: 'steam_enabled',
    epic: 'library_epic_enabled',
    battlenet: 'library_battlenet_enabled',
    ea: 'library_ea_enabled',
    ubisoft: 'library_ubisoft_enabled',
    gog: 'library_gog_enabled',
    microsoft: 'library_microsoft_enabled',
    // Game folders are on when there are folders to read.
    local: null,
  };
  /** Shown order: the big launchers first, game folders last. */
  export const LAUNCHER_ORDER: LibrarySource[] = ['steam', 'epic', 'battlenet', 'microsoft', 'ea', 'ubisoft', 'gog', 'local'];
  /** Short monograms for the launcher tiles. */
  const MONOGRAM: Record<LibrarySource, string> = {
    steam: 'St', epic: 'Ep', battlenet: 'Bn', ea: 'EA', ubisoft: 'Ub', gog: 'GOG', microsoft: 'Xb', local: '',
  };
  export const monogram = (source: LibrarySource) => MONOGRAM[source];

  export type Source = LibraryDiscovery['sources'][number];
  /** One line for a launcher, in the same words for every launcher. */
  export function describe(source: Source, games: LibraryDiscovery['games']): { text: string; tone: 'ok' | 'muted' | 'warn' } {
    const mine = games.filter((game) => game.source === source.source);
    const added = mine.filter((game) => game.already_added).length;
    if (source.state === 'disabled') {
      return { text: source.source === 'local' ? 'No game folders chosen' : 'Off', tone: 'muted' };
    }
    if (source.state === 'unavailable' && mine.length === 0) return { text: 'Not installed on this PC', tone: 'muted' };
    if (source.state === 'error') {
      return { text: `Some games could not be read${mine.length ? `; ${added} of ${mine.length} in your library` : ''}`, tone: 'warn' };
    }
    if (mine.length === 0) return { text: 'No games installed', tone: 'muted' };
    const word = mine.length === 1 ? 'game' : 'games';
    return { text: added === mine.length ? `${mine.length} ${word}, all in your library` : `${added} of ${mine.length} ${word} in your library`, tone: 'ok' };
  }
</script>

<script lang="ts">
  // Where the library's games come from: every launcher in one list, with the same switch,
  // the same status words and one Sync now, Steam no different from the others.
  import { onMount } from 'svelte';
  import Button from '../Button.svelte';
  import Dialog from '../Dialog.svelte';
  import Icon from '../Icon.svelte';
  import Panel from '../Panel.svelte';
  import Switch from '../settings/Switch.svelte';
  import { api, type Config, type PlayniteStatus } from '../../lib/api';
  import { failed, notify } from '../../lib/feedback.svelte';
  import { link } from '../../lib/router.svelte';
  import { counted, removedGames, restoreMessage, toggled } from './selection.svelte';

  let {
    removed = [],
    onsynced,
  }: {
    /** The removed games' `launcher-dismissed` keys, from the app list. */
    removed?: string[];
    onsynced?: () => void;
  } = $props();

  let catalog = $state<LibraryDiscovery | null>(null);
  let config = $state<Config>({});
  let playnite = $state<PlayniteStatus | null>(null);
  let error = $state('');
  let loading = $state(true);
  let syncing = $state(false);
  let saving = $state<string | null>(null);

  const rows = $derived(
    LAUNCHER_ORDER.map((id) => catalog?.sources.find((source) => source.source === id)).filter((source): source is Source => Boolean(source)),
  );
  const flag = (key: string, fallback: boolean) => {
    const value = config[key];
    if (value === undefined || value === null || value === '') return fallback;
    return !['false', 'no', '0', 'off', 'disabled'].includes(String(value).trim().toLowerCase());
  };
  const automatic = $derived(flag('library_auto_sync', true) && flag('steam_auto_sync', true));

  async function load() {
    loading = true;
    try {
      const [found, stored, plugin] = await Promise.all([
        api.library.discovery(),
        api.config.get(),
        api.playnite.status().catch(() => null),
      ]);
      // A host that has not read its launchers yet answers without lists.
      catalog = { ...found, sources: found.sources ?? [], games: found.games ?? [] };
      config = stored;
      playnite = plugin;
      error = '';
    } catch (reason) {
      error = reason instanceof Error ? reason.message : String(reason);
    } finally {
      loading = false;
    }
  }
  onMount(() => {
    void load();
  });

  async function toggle(key: string, on: boolean) {
    saving = key;
    try {
      await api.config.patch({ [key]: on });
      config = { ...config, [key]: on };
      if (on) await syncAll(false);
      await load();
    } catch (reason) {
      failed('Saving the launcher setting failed', reason);
    } finally {
      saving = null;
    }
  }

  // Restore removed games: all ticked to start with, the usual wish after a change of mind.
  let restoreOpen = $state(false);
  let restoring = $state(false);
  let restorePicked = $state.raw<Set<string>>(new Set());
  const removedList = $derived(removedGames(removed, catalog?.games ?? []));
  const restoreKeys = $derived(removed.filter((key) => restorePicked.has(key)));

  function openRestore() {
    restorePicked = new Set(removed);
    restoreOpen = true;
  }

  async function restore() {
    if (restoreKeys.length === 0) return;
    restoring = true;
    try {
      const { restored } = await api.library.restore(restoreKeys);
      restoreOpen = false;
      const synced = await syncAll(false);
      // The removed list is shorter now, whatever the sync did.
      if (!synced) onsynced?.();
      const message = restoreMessage(restored, synced, automatic);
      notify(message.text, message.tone);
    } catch (reason) {
      failed('Restoring the games failed', reason);
    } finally {
      restoring = false;
    }
  }

  /** Playnite's line, with the plugin version: the details its own bar used to show. */
  const playniteState = $derived.by(() => {
    if (!playnite?.enabled) return 'Off';
    if (!playnite.installed) return 'Needs the Rubylight plugin in Playnite';
    const plugin = playnite.installed_version ? ` · plugin ${playnite.installed_version}` : '';
    if (!playnite.active) return `Syncs while Playnite runs${plugin}`;
    return `${counted(playnite.game_count, 'game')}, kept in step while Playnite runs${plugin}`;
  });

  // The Rubylight plugin is how Playnite hands over its games; its row installs and updates it.
  let installing = $state(false);
  async function installPlugin() {
    installing = true;
    try {
      const result = await api.playnite.install();
      notify(
        result.restart_required ? 'Plugin installed. Restart Playnite to load it.' : 'Plugin installed. It loads when Playnite starts.',
        'ok',
      );
      await load();
    } catch (reason) {
      failed('Installing the Playnite plugin failed', reason);
    } finally {
      installing = false;
    }
  }

  /** Whether the launchers synced; a failure is shown here. */
  async function syncAll(announce = true): Promise<boolean> {
    syncing = true;
    try {
      const [steam, launchers] = await Promise.all([
        api.steam.sync().catch(() => null),
        api.library.sync(),
        playnite?.active ? api.playnite.sync().catch(() => null) : null,
      ]);
      onsynced?.();
      if (announce) {
        const changed = (steam?.changed ?? false) || launchers.added + launchers.removed > 0;
        notify(changed ? 'Your library is up to date with your launchers.' : 'Nothing new: your library already has every game.', 'ok');
      }
      await load();
      return true;
    } catch (reason) {
      failed('Syncing the launchers failed', reason);
      return false;
    } finally {
      syncing = false;
    }
  }
</script>

<Panel title="Where your games come from" description="Games installed with these launchers join your library by themselves, with their covers.">
  {#snippet actions()}
    <Button size="sm" icon="refresh" busy={syncing} disabled={loading && !catalog} onclick={() => syncAll()}>Sync now</Button>
  {/snippet}
  {#if error}
    <p class="notice danger" role="alert">Could not read the launchers: {error} <Button size="sm" onclick={load}>Retry</Button></p>
  {/if}
  {#if !catalog && loading}
    <ul class="sources" aria-busy="true" aria-label="Launchers">
      {#each LAUNCHER_ORDER as id (id)}<li class="source placeholder"><span class="tile"></span><span class="line"></span></li>{/each}
    </ul>
  {:else if catalog}
    <ul class="sources" aria-label="Launchers">
      {#each rows as source (source.source)}
        {@const state = describe(source, catalog.games)}
        {@const key = LAUNCHER_KEYS[source.source]}
        <li class="source" class:off={source.state === 'disabled'}>
          <span class="tile" aria-hidden="true">{#if source.source === 'local'}<Icon name="folder" size={18} />{:else}{monogram(source.source)}{/if}</span>
          <span class="text">
            <!-- Steam's link sits by its name so the status line keeps the row's width. -->
            <span class="name">
              <strong id="launcher-{source.source}">{source.name}</strong>
              {#if source.source === 'steam'}<a class="choose" href="/settings/library" use:link>Steam options</a>{/if}
            </span>
            <span class="state {state.tone}">{#if state.tone === 'warn'}<Icon name="alert" size={14} />{/if}{state.text}</span>
          </span>
          {#if key}
            <label class="switch-label" title="{source.name}: {source.state === 'disabled' ? 'off' : 'on'}">
              <span class="visually-hidden">Add games from {source.name}</span>
              <Switch checked={source.state !== 'disabled'} describedby="launcher-{source.source}" onchange={(on) => toggle(key, on)} />
            </label>
          {:else}
            <a class="choose" href="/settings/library" use:link>Choose folders</a>
          {/if}
          {#if saving === key && key}<span class="visually-hidden" role="status">Saving…</span>{/if}
        </li>
      {/each}
      {#if playnite?.available || playnite?.installed}
        <li class="source" class:off={!playnite.enabled}>
          <span class="tile" aria-hidden="true">Pn</span>
          <span class="text">
            <span class="name">
              <strong id="launcher-playnite">Playnite</strong>
              <a class="choose" href="/settings/library" use:link>Playnite options</a>
            </span>
            <span class="state {playnite.enabled ? (playnite.installed ? 'ok' : 'warn') : 'muted'}">
              {playniteState}
            </span>
            {#if playnite.enabled && (!playnite.installed || playnite.update_available)}
              <span class="plugin">
                <Button size="sm" icon="download" busy={installing} disabled={syncing} onclick={installPlugin}>
                  {playnite.installed ? 'Update plugin' : 'Install plugin'}
                </Button>
              </span>
            {/if}
          </span>
          <label class="switch-label">
            <span class="visually-hidden">Add games from Playnite</span>
            <Switch checked={playnite.enabled} describedby="launcher-playnite" onchange={(on) => toggle('playnite_enabled', on)} />
          </label>
        </li>
      {/if}
    </ul>
    <p class="foot muted">
      {automatic ? 'Every installed game is added by itself and removed when you uninstall it.' : 'Automatic sync is off: select Sync now after installing a game.'}
      {#if playnite?.active}While Playnite runs, it adds the games of every launcher.{/if}
      Pick single games with <a href="/library/import" use:link>Add games</a>; more choices are in <a href="/settings/library" use:link>All settings › Games</a>.
    </p>
    {#if removed.length > 0}
      <p class="foot muted removed">
        <span>{counted(removed.length, 'game')} you removed {removed.length === 1 ? 'stays' : 'stay'} out of your library.</span>
        <Button size="sm" icon="refresh" disabled={syncing} onclick={openRestore}>Restore removed games</Button>
      </p>
    {/if}
  {/if}
</Panel>

<Dialog
  bind:open={restoreOpen}
  title="Restore removed games"
  description={automatic
    ? 'The games you pick join your library again if they are still installed.'
    : 'Automatic sync is off: the games you pick can be added again with Add games.'}
  dismissible={!restoring}
>
  <div class="pick-all">
    <Button size="sm" variant="ghost" onclick={() => (restorePicked = new Set(removed))}>Select all</Button>
    <Button size="sm" variant="ghost" disabled={restoreKeys.length === 0} onclick={() => (restorePicked = new Set())}>Select none</Button>
  </div>
  <ul class="removed-list" aria-label="Removed games">
    {#each removedList as game (game.key)}
      <li>
        <label class="check">
          <input type="checkbox" checked={restorePicked.has(game.key)} onchange={() => (restorePicked = toggled(restorePicked, game.key))} />
          <span>{game.name} <span class="muted">· {game.launcher}</span></span>
        </label>
      </li>
    {/each}
  </ul>
  {#snippet actions()}
    <Button disabled={restoring} onclick={() => (restoreOpen = false)}>Cancel</Button>
    <Button variant="primary" busy={restoring} disabled={restoreKeys.length === 0} onclick={restore}>
      Restore {counted(restoreKeys.length, 'game')}
    </Button>
  {/snippet}
</Dialog>

<style>
  .sources {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(300px, 1fr));
    gap: var(--space-2) var(--space-5);
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .source {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    min-height: 60px;
    padding: var(--space-2) 0;
    border-bottom: 1px solid var(--line);
  }
  .tile {
    flex: none;
    display: grid;
    place-items: center;
    width: 40px;
    height: 40px;
    border-radius: var(--radius-sm);
    background: var(--accent-soft);
    color: var(--accent-text);
    font-weight: var(--weight-bold);
    font-size: var(--text-xs);
    letter-spacing: 0.02em;
  }
  .off .tile {
    background: var(--sunken);
    color: var(--muted);
  }
  .text {
    flex: 1;
    min-width: 0;
    display: grid;
    gap: 2px;
  }
  .text strong {
    font-weight: var(--weight-medium);
  }
  .name {
    display: flex;
    align-items: baseline;
    flex-wrap: wrap;
    gap: 0 var(--space-3);
  }
  .state {
    display: inline-flex;
    align-items: center;
    gap: var(--space-1);
    font-size: var(--text-sm);
    color: var(--muted);
  }
  .state.ok {
    color: var(--ink-2);
  }
  .state.warn {
    color: var(--warn);
  }
  .plugin {
    display: flex;
    margin-top: var(--space-1);
  }
  .switch-label {
    display: inline-flex;
    align-items: center;
    min-height: var(--control-height);
    cursor: pointer;
  }
  .choose {
    font-size: var(--text-sm);
    white-space: nowrap;
  }
  .placeholder .tile {
    background: var(--skeleton);
  }
  .placeholder .line {
    flex: 1;
    height: 14px;
    max-width: 180px;
    border-radius: var(--radius-sm);
    background: var(--skeleton);
  }
  .foot {
    margin-top: var(--space-4);
    font-size: var(--text-sm);
  }
  .notice {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    flex-wrap: wrap;
  }
  .removed {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    flex-wrap: wrap;
    margin-top: var(--space-2);
  }
  .pick-all {
    display: flex;
    gap: var(--space-2);
  }
  .removed-list {
    display: grid;
    gap: var(--space-2);
    max-height: 50vh;
    overflow-y: auto;
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .check {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    font-size: var(--text-sm);
    cursor: pointer;
  }
  .check input {
    margin: 0;
    accent-color: var(--ink-2);
  }
</style>
