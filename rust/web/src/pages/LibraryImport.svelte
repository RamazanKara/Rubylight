<script lang="ts">
  import { onMount } from 'svelte';
  import Button from '../components/Button.svelte';
  import PageHeader from '../components/PageHeader.svelte';
  import Panel from '../components/Panel.svelte';
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

<a class="back" href="/library" use:link>‹ Games &amp; apps</a>
<PageHeader title="Add games" subtitle="Choose games installed on this PC. Their launchers will open when you play.">
  {#snippet actions()}<Button href="/library/new">Add manually</Button>{/snippet}
</PageHeader>
<Panel title="Installed games" description="Steam, Epic Games, Battle.net, EA app, Ubisoft Connect, and GOG.">
  {#snippet actions()}<Button onclick={scan} disabled={scanning || importing} busy={scanning}>Scan again</Button>{/snippet}
  <div class="filters">
    <label>Search games<input class="input" type="search" placeholder="Find an installed game…" bind:value={search} /></label>
    <label>Launcher<select class="select" aria-label="Launcher" bind:value={source}>
      <option value="all">All launchers</option>
      {#each catalog?.sources ?? [] as launcher (launcher.source)}<option value={launcher.source}>{launcher.name}</option>{/each}
    </select></label>
  </div>
  {#if scanning}<p role="status">Looking for games installed on this PC…</p>{/if}
  {#if error}<p class="error" role="alert">{error} Scan again to refresh the list.</p>{/if}
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
          <li class:unavailable={Boolean(game.unavailable_reason)}>
            <label class="game">
              <input type="checkbox" aria-label={'Add ' + game.name + ' from ' + (catalog.sources.find(item => item.source === game.source)?.name ?? game.source)} checked={selected.includes(key(game))} disabled={!canAdd(game) || importing} onchange={event => toggle(game, event.currentTarget.checked)} />
              <span class="cover"><AppCover name={game.name} uuid={game.uuid} path={game.has_cover ? 'installed' : ''} /></span>
              <span class="game-name"><strong>{game.name}</strong><span class="muted">{catalog.sources.find(item => item.source === game.source)?.name ?? game.source}</span>{#if game.unavailable_reason}<span class="error">{game.unavailable_reason}</span>{/if}</span>
              <span class="game-state">{game.already_added ? 'Already added' : game.unavailable_reason ? 'Needs attention' : 'Installed'}</span>
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
        <div class="source"><strong>{launcher.name}</strong><span>{launcher.state === 'error' ? 'Scan incomplete' : launcher.state === 'unavailable' ? 'Launcher not found' : catalog.games.some(game => game.source === launcher.source) ? catalog.games.filter(game => game.source === launcher.source).length + ' installed' : 'No installed games'}</span></div>
        {#each launcher.errors as message}<p class="error">{message}</p>{/each}
      {/each}
    </details>
  {/if}
</Panel>
<div class="import-bar" class:has-selection={selected.length > 0}><p>{selected.length ? selected.length + ' selected' : 'Choose the games you want to add.'}<span class="muted">You can change names and covers later.</span></p><Button variant="primary" disabled={scanning || !selected.length || !catalog} busy={importing} onclick={importGames}>{importing ? 'Adding games…' : 'Add selected games'}</Button></div>
<p class="muted source-help">Automatic Steam and Playnite synchronization is available in <a href="/library" use:link>Manage sources</a>.</p>

<style>
  .back { width: fit-content; min-height: 44px; display: inline-flex; align-items: center; color: var(--muted); text-decoration: none; }
  .filters { display: grid; grid-template-columns: minmax(180px, 1fr) minmax(180px, 240px); gap: 20px; margin-bottom: 20px; }
  .filters label { display: grid; gap: 8px; }
  .games { list-style: none; margin: 0; padding: 0; }
  .games li { border-top: 1px solid var(--line); }
  .game { display: flex; gap: 18px; align-items: center; padding: 16px 0; cursor: pointer; }
  input[type='checkbox'] { flex: 0 0 20px; width: 20px; height: 20px; accent-color: var(--accent); }
  .cover { width: 52px; flex: 0 0 52px; }
  .cover :global(.initials) { font-size: 20px; }
  .game-name { display: grid; gap: 4px; flex: 1; min-width: 0; overflow-wrap: anywhere; }
  .game-name .muted, .game-state { font-size: var(--text-sm); }
  .game-state { color: var(--muted); }
  .select-all { display: flex; align-items: center; gap: 18px; min-height: 56px; }
  .select-all span { margin-left: auto; font-size: var(--text-sm); }
  .empty { display: grid; justify-items: start; gap: 16px; padding: 32px 0; }
  .sources { margin-top: 24px; color: var(--muted); }
  .source { display: flex; justify-content: space-between; gap: 16px; padding: 8px 0; }
  .sources .error { overflow-wrap: anywhere; }
  .import-bar.has-selection { position: sticky; bottom: 16px; }
  .import-bar { display: flex; justify-content: space-between; align-items: center; gap: 24px; padding: 20px 24px; border: 1px solid var(--line); border-radius: var(--radius-lg); background: var(--panel); box-shadow: var(--shadow); }
  .import-bar p { display: grid; gap: 4px; }
  .import-bar .muted, .source-help { font-size: var(--text-sm); }
  .result { display: grid; gap: 8px; padding: 16px 0; }
  @media (max-width: 600px) {
    .filters { grid-template-columns: 1fr; }
    .game { display: grid; grid-template-columns: 20px 52px minmax(0, 1fr); gap: 4px 12px; }
    .game > input, .game > .cover { grid-row: span 2; }
    .game-state { grid-column: 3; }
    .select-all { flex-wrap: wrap; gap: 8px; }
    .select-all span { margin-left: 0; }
    .import-bar { padding: 16px; gap: 12px; }
    .import-bar.has-selection { bottom: 8px; }
    .import-bar .muted { display: none; }
  }
</style>
