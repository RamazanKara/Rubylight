<script lang="ts">
  import { onMount } from 'svelte';
  import Button from '../components/Button.svelte';
  import PageHeader from '../components/PageHeader.svelte';
  import Panel from '../components/Panel.svelte';
  import Badge from '../components/Badge.svelte';
  import Icon from '../components/Icon.svelte';
  import AppCover from '../components/library/AppCover.svelte';
  import { api, type DiscoveredGame, type LibraryDiscovery, type LibraryImportResult } from '../lib/api';
  import { link } from '../lib/router.svelte';

  let catalog = $state<LibraryDiscovery | null>(null);
  let scanning = $state(false);
  let importing = $state(false);
  let error = $state('');
  let search = $state('');
  let source = $state('all');
  let selected = $state<string[]>([]);
  let results = $state<LibraryImportResult[]>([]);
  const key = (game: { source: string; id: string }) => JSON.stringify([game.source, game.id]);
  const canAdd = (game: DiscoveredGame) => !game.already_added && !game.unavailable_reason;
  const visible = $derived((catalog?.games ?? []).filter(game =>
    (source === 'all' || game.source === source) && game.name.toLocaleLowerCase().includes(search.trim().toLocaleLowerCase())));
  const selectable = $derived(visible.filter(canAdd));
  const allSelected = $derived(selectable.length > 0 && selectable.every(game => selected.includes(key(game))));
  const added = $derived(results.filter(result => result.result === 'added').length);
  const existing = $derived(results.filter(result => result.result === 'already_present').length);
  const failures = $derived(results.filter(result => result.result === 'failed'));
  const scanned = $derived((catalog?.sources ?? []).filter(launcher => launcher.state !== 'disabled'));
  function sourceState(launcher: LibraryDiscovery['sources'][number]): string {
    const count = catalog?.games.filter(game => game.source === launcher.source).length ?? 0;
    if (launcher.state === 'disabled') return launcher.source === 'local' ? 'No game folders chosen' : 'Turned off in Library settings';
    if (launcher.state === 'error') return 'Scan incomplete';
    if (launcher.state === 'unavailable') return 'Launcher not found';
    return count ? count + ' installed' : 'No installed games';
  }

  async function scan() {
    if (scanning || importing) return;
    scanning = true;
    error = '';
    results = [];
    try {
      catalog = await api.library.discovery();
      selected = selected.filter(id => catalog!.games.some(game => key(game) === id && canAdd(game)));
    } catch (reason) {
      error = reason instanceof Error ? reason.message : String(reason);
      catalog = null;
      selected = [];
    } finally {
      scanning = false;
    }
  }
  function toggle(game: DiscoveredGame, checked: boolean) {
    selected = checked ? [...selected, key(game)] : selected.filter(id => id !== key(game));
  }
  function toggleVisible(checked: boolean) {
    const ids = selectable.map(key);
    selected = checked ? [...new Set([...selected, ...ids])] : selected.filter(id => !ids.includes(id));
  }
  async function importGames() {
    if (!catalog || importing || scanning || !selected.length) return;
    importing = true;
    error = '';
    results = [];
    try {
      const reply = await api.library.import(catalog.games.filter(game => selected.includes(key(game))).map(({ source, id }) => ({ source, id })));
      results = reply.results;
      const complete = results.filter(result => result.result !== 'failed').map(key);
      catalog = { ...catalog, games: catalog.games.map(game => complete.includes(key(game)) ? { ...game, already_added: true } : game) };
      selected = selected.filter(id => !complete.includes(id));
    } catch (reason) {
      error = reason instanceof Error ? reason.message : String(reason);
    } finally {
      importing = false;
    }
  }
  onMount(() => { void scan(); });
</script>

<a class="back" href="/library" use:link><span class="back-icon"><Icon name="chevron" size={16} /></span>Games</a>
<PageHeader title="Add games" subtitle="Choose games installed on this PC. Their launchers will open when you play.">
  {#snippet actions()}<Button href="/library/new">Add manually</Button>{/snippet}
</PageHeader>
<Panel title="Installed games" description="Steam, Epic Games, Battle.net, EA app, Ubisoft Connect, GOG, Microsoft Store and Xbox PC games, and the game folders you choose.">
  {#snippet actions()}<Button onclick={scan} disabled={scanning || importing} busy={scanning}>Scan again</Button>{/snippet}
  <div class="filters">
    <label><span class="label">Search games</span><span class="search"><span class="search-icon"><Icon name="search" size={16} /></span><input class="input" type="search" placeholder="Find an installed game…" bind:value={search} /></span></label>
    <label><span class="label">Launcher</span><select class="select" aria-label="Launcher" bind:value={source}>
      <option value="all">All launchers</option>
      {#each scanned as launcher (launcher.source)}<option value={launcher.source}>{launcher.name}</option>{/each}
    </select></label>
  </div>
  {#if scanning}<p class="scanning muted" role="status"><span class="spinner" aria-hidden="true"></span>Looking for games installed on this PC…</p>{/if}
  {#if error}<p class="notice danger" role="alert">{error} Scan again to refresh the list.</p>{/if}
  {#if results.length}
    <div class="result" role="status">
      <p>{added} {added === 1 ? 'game' : 'games'} added.{existing ? ' ' + existing + ' already in your library.' : ''}</p>
      {#if added || existing}<a href="/library" use:link>View library</a>{/if}
      {#each failures as result (key(result))}<p class="error">{catalog?.games.find(game => key(game) === key(result))?.name ?? result.id}: {result.error}</p>{/each}
    </div>
  {/if}
  {#if catalog && !scanning}
    {#if visible.length}
      <label class="select-all"><input type="checkbox" checked={allSelected} disabled={!selectable.length || importing} onchange={event => toggleVisible(event.currentTarget.checked)} />Select all available games shown<span class="muted">{visible.length} found</span></label>
      <ul class="games">
        {#each visible as game (key(game))}
          <li class:unavailable={Boolean(game.unavailable_reason)} class:added={game.already_added}>
            <label class="game">
              <input type="checkbox" aria-label={'Add ' + game.name + ' from ' + (catalog.sources.find(item => item.source === game.source)?.name ?? game.source)} checked={selected.includes(key(game))} disabled={!canAdd(game) || importing} onchange={event => toggle(game, event.currentTarget.checked)} />
              <span class="cover"><AppCover name={game.name} uuid={game.uuid} path={game.has_cover ? 'installed' : ''} /></span>
              <span class="game-name"><strong>{game.name}</strong><span class="muted">{catalog.sources.find(item => item.source === game.source)?.name ?? game.source}</span>{#if game.unavailable_reason}<span class="error">{game.unavailable_reason}</span>{/if}</span>
              <span class="game-state">{#if game.already_added}<Badge>Already added</Badge>{:else if game.unavailable_reason}<Badge tone="warn">Needs attention</Badge>{:else}Installed{/if}</span>
            </label>
          </li>
        {/each}
      </ul>
    {:else}
      <div class="empty"><h3>{search.trim() ? 'No matching games' : 'No installed games found'}</h3><p class="muted">{search.trim() ? 'Try a different name or launcher.' : 'Install a game in its launcher, then scan again. You can also add a program or Desktop manually.'}</p><Button href="/library/new">Add manually</Button></div>
    {/if}
    <details class="sources" open={catalog.sources.some(launcher => launcher.state === 'error')}>
      <summary>Scan results by launcher</summary>
      {#each catalog.sources as launcher (launcher.source)}
        <div class="source"><strong>{launcher.name}</strong><span>{sourceState(launcher)}</span></div>
        {#each launcher.errors as message}<p class="error">{message}</p>{/each}
      {/each}
    </details>
  {/if}
</Panel>
<div class="import-bar" class:has-selection={selected.length > 0}><p>{selected.length ? selected.length + ' selected' : 'Choose the games you want to add.'}<span class="muted">You can change names and covers later.</span></p><Button variant="primary" disabled={scanning || !selected.length || !catalog} busy={importing} onclick={importGames}>{importing ? 'Adding games…' : 'Add selected games'}</Button></div>
<p class="muted source-help">Launchers can add every installed game by themselves: turn them on under <a href="/library" use:link>Where your games come from</a>. Game folders are chosen in <a href="/settings/library" use:link>All settings › Games</a>.</p>

<style>
  .back { justify-self: start; width: fit-content; min-height: var(--control-height); display: inline-flex; align-items: center; gap: var(--space-1); font-size: var(--text-sm); color: var(--ink-2); text-decoration: none; }
  .back:hover { color: var(--ink); text-decoration: underline; }
  .back-icon { display: inline-flex; transform: rotate(180deg); }
  .filters { display: grid; grid-template-columns: minmax(180px, 1fr) minmax(180px, 260px); gap: var(--space-4) var(--space-5); margin-bottom: var(--space-3); }
  .filters label { display: grid; gap: 6px; align-content: start; }
  .label { font-size: var(--text-sm); font-weight: var(--weight-medium); }
  .search { position: relative; }
  .search-icon { position: absolute; left: var(--space-3); top: 50%; transform: translateY(-50%); color: var(--muted); pointer-events: none; }
  .search .input { padding-left: 38px; }
  .scanning { display: flex; align-items: center; gap: var(--space-3); padding: var(--space-5) 0; font-size: var(--text-sm); }
  .spinner { width: 18px; height: 18px; border: 2px solid var(--line-strong); border-top-color: var(--accent); border-radius: var(--radius-pill); animation: spin 0.8s linear infinite; }
  @keyframes spin { to { transform: rotate(360deg); } }
  @media (prefers-reduced-motion: reduce) { .spinner { animation: none; } }
  .games { list-style: none; margin: 0; padding: 0; }
  .games li { border-top: 1px solid var(--line); }
  .game { display: flex; gap: var(--space-4); align-items: center; min-height: 72px; padding: var(--space-3) var(--space-2); margin: 0 calc(-1 * var(--space-2)); border-radius: var(--radius); cursor: pointer; transition: background-color var(--duration-fast) var(--ease-standard); }
  .game:hover { background: var(--sunken); }
  .added .game { cursor: default; }
  .added .game:hover { background: none; }
  .added .cover { opacity: 0.6; }
  input[type='checkbox'] { flex: 0 0 20px; width: 20px; height: 20px; margin: 0; accent-color: var(--accent); }
  .cover { width: 40px; flex: 0 0 40px; }
  .game-name { display: grid; gap: 2px; flex: 1; min-width: 0; overflow-wrap: anywhere; }
  .game-name strong { font-weight: var(--weight-semibold); }
  .game-name .muted, .game-state { font-size: var(--text-sm); }
  .game-name .error { color: var(--danger); font-size: var(--text-sm); }
  .game-state { color: var(--muted); white-space: nowrap; }
  .select-all { display: flex; align-items: center; gap: var(--space-4); min-height: var(--control-height); font-size: var(--text-sm); font-weight: var(--weight-medium); cursor: pointer; }
  .select-all span { margin-left: auto; font-weight: var(--weight-regular); }
  .empty { display: grid; justify-items: start; gap: var(--space-3); padding: var(--space-6) 0; }
  .empty h3 { font-size: var(--text-md); }
  .sources { margin-top: var(--space-5); padding-top: var(--space-4); border-top: 1px solid var(--line); color: var(--muted); font-size: var(--text-sm); }
  .sources summary { padding: var(--space-2) 0; cursor: pointer; color: var(--ink-2); font-weight: var(--weight-medium); }
  .source { display: flex; justify-content: space-between; gap: var(--space-4); padding: var(--space-2) 0; border-bottom: 1px solid var(--line); }
  .source strong { color: var(--ink-2); font-weight: var(--weight-medium); }
  .sources .error { color: var(--danger); overflow-wrap: anywhere; }
  .import-bar.has-selection { position: sticky; bottom: var(--space-4); z-index: var(--z-sticky); box-shadow: var(--shadow-pop); border-color: var(--line-strong); }
  .import-bar { display: flex; justify-content: space-between; align-items: center; gap: var(--space-5); padding: var(--space-4) var(--space-5); border: 1px solid var(--line); border-radius: var(--radius-lg); background: var(--panel); box-shadow: var(--shadow); }
  .import-bar p { display: grid; gap: 2px; font-weight: var(--weight-medium); }
  .import-bar .muted, .source-help { font-size: var(--text-sm); font-weight: var(--weight-regular); }
  .result { display: grid; gap: var(--space-2); justify-items: start; margin: var(--space-2) 0 var(--space-4); padding: var(--space-3) var(--space-4); border: 1px solid var(--line); border-radius: var(--radius); background: var(--sunken); color: var(--ink); }
  .result .error { color: var(--danger); }
  @media (max-width: 600px) {
    .filters { grid-template-columns: 1fr; }
    .game { display: grid; grid-template-columns: 20px 40px minmax(0, 1fr); gap: var(--space-1) var(--space-3); }
    .game > input, .game > .cover { grid-row: span 2; }
    .game-state { grid-column: 3; }
    .import-bar { padding: var(--space-3) var(--space-3) var(--space-3) var(--space-4); gap: var(--space-3); }
    .import-bar.has-selection { bottom: var(--space-2); }
    .import-bar .muted { display: none; }
  }
</style>
