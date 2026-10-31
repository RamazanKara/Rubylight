<script lang="ts">
  import { onMount } from 'svelte';
  import Button from '../components/Button.svelte';
  import EmptyState from '../components/EmptyState.svelte';
  import PageHeader from '../components/PageHeader.svelte';
  import Skeleton from '../components/Skeleton.svelte';
  import Panel from '../components/Panel.svelte';
  import AppEditor from '../components/library/AppEditor.svelte';
  import LibraryGrid from '../components/library/LibraryGrid.svelte';
  import LauncherSources from '../components/library/LauncherSources.svelte';
  import PlayniteSource from '../components/library/PlayniteSource.svelte';
  import { api, type App, type SessionStatus } from '../lib/api';
  import { failed } from '../lib/feedback.svelte';
  import { poll } from '../lib/format';
  import { match, navigate } from '../lib/router.svelte';

  let apps = $state<App[] | null>(null);
  /** The synced games removed from the library, which a restore can bring back. */
  let removed = $state<string[]>([]);
  let loadError = $state('');
  let status = $state<SessionStatus | null>(null);
  let reordering = $state(false);
  let latestLoad = 0;

  /** null on the grid, 'new' for a new app, otherwise the app's uuid. */
  const editing = $derived(match('/library/new') ? 'new' : (match('/library/:uuid')?.uuid ?? null));

  const reason = (error: unknown) => (error instanceof Error ? error.message : String(error));

  async function load() {
    // A slow reload must not put an older list back.
    const id = ++latestLoad;
    try {
      const library = await api.apps.list();
      if (id !== latestLoad) return;
      apps = library.apps ?? [];
      removed = library['launcher-dismissed'] ?? [];
      loadError = '';
    } catch (error) {
      if (id === latestLoad) loadError = reason(error);
    }
  }

  async function refreshStatus() {
    try {
      status = await api.sessions.status();
    } catch {
      // The Running badge keeps its last state.
    }
  }

  onMount(() => {
    void load();
    return poll(refreshStatus, 5000);
  });

  async function reorder(next: App[]) {
    if (reordering) return;
    reordering = true;
    apps = next;
    try {
      await api.apps.reorder(next.flatMap((app) => (app.uuid ? [app.uuid] : [])));
    } catch (error) {
      failed('Saving the order failed', error);
      await load();
    } finally {
      reordering = false;
    }
  }

  const current = $derived(editing && editing !== 'new' ? apps?.find((app) => app.uuid === editing) : undefined);
  // Moving needs every app's uuid; the host puts apps it is not given at the end.
  const position = $derived.by(() => {
    if (!apps || !current || apps.some((app) => !app.uuid)) return null;
    return { index: apps.findIndex((app) => app.uuid === current.uuid), count: apps.length };
  });

  async function move(uuid: string, delta: -1 | 1) {
    if (!apps) return;
    const from = apps.findIndex((app) => app.uuid === uuid);
    const to = from + delta;
    if (from < 0 || to < 0 || to >= apps.length) return;
    const next = apps.slice();
    const [moved] = next.splice(from, 1);
    if (moved) next.splice(to, 0, moved);
    await reorder(next);
  }

  async function saved(uuid: string, created: boolean) {
    await load();
    if (created) navigate(`/library/${encodeURIComponent(uuid)}`, { replace: true });
  }

  function deleted(uuid: string) {
    // The app is gone, so its unsaved edits are too: no "Discard changes?" on the way out.
    navigate('/library', { force: true });
    if (apps) apps = apps.filter((app) => app.uuid !== uuid);
    void load();
  }
</script>

{#snippet addApp()}
  <Button variant="primary" icon="plus" href="/library/import">Add games</Button>
  <Button href="/library/new">Add manually</Button>
{/snippet}

{#if editing === null}
  <PageHeader title="Games" subtitle="Everything your phone or TV can start on this PC." actions={apps?.length === 0 ? undefined : addApp} />

  {#if loadError}
    <div class="notice danger load-error" role="alert">
      <span>{apps ? 'Could not refresh the library' : 'Could not load the library'}: {loadError}</span>
      <Button size="sm" onclick={load}>Retry</Button>
    </div>
  {/if}


  {#if apps === null}
    {#if !loadError}
      <div class="loading">
        <Skeleton shape="block" width="min(380px, 100%)" height="var(--control-height)" label="Loading your games" />
        <ul class="covers" aria-hidden="true">
          {#each [0, 1, 2, 3, 4, 5] as index (index)}
            <li><Skeleton shape="block" height="100%" /><Skeleton width="70%" /></li>
          {/each}
        </ul>
      </div>
    {/if}
  {:else if apps.length === 0}
    <Panel>
      <EmptyState icon="library" title="No games yet">
        <p>Games from the launchers below appear here by themselves. You can also pick games, or add any program.</p>
        <Button variant="primary" icon="plus" href="/library/import">Add games</Button>
        <Button href="/library/new">Add manually</Button>
      </EmptyState>
    </Panel>
  {:else}
    <LibraryGrid {apps} {status} {reordering} onreorder={reorder} onstatus={refreshStatus} onremoved={load} />
  {/if}
  <LauncherSources {removed} onsynced={load} />
  <PlayniteSource onsynced={load} />
{:else}
  {#key editing}
    {#if editing === 'new'}
      <AppEditor
        position={null}
        {status}
        {reordering}
        onsaved={saved}
        onmove={move}
        ondeleted={deleted}
        onstatus={refreshStatus}
      />
    {:else if current}
      <AppEditor
        app={current}
        {position}
        {status}
        {reordering}
        onsaved={saved}
        onmove={move}
        ondeleted={deleted}
        onstatus={refreshStatus}
      />
    {:else if apps === null}
      {#if loadError}
        <div class="notice danger load-error" role="alert">
          <span>Could not load the app: {loadError}</span>
          <Button size="sm" onclick={load}>Retry</Button>
        </div>
      {:else}
        <p class="muted placeholder">Loading the app…</p>
      {/if}
    {:else}
      <Panel>
        <EmptyState icon="library" title="App not found">
          <p>It may have been deleted.</p>
          <Button href="/library">Back to Games</Button>
        </EmptyState>
      </Panel>
    {/if}
  {/key}
{/if}

<style>
  .placeholder {
    font-size: var(--text-sm);
  }
  .loading {
    display: grid;
    gap: var(--space-4);
  }
  /* The same columns and 2:3 covers as the grid, so nothing jumps when it arrives. */
  .covers {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(144px, 1fr));
    gap: var(--space-5) var(--space-4);
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .covers li {
    display: grid;
    gap: var(--space-2);
    grid-template-rows: auto auto;
  }
  .covers li > :global(:first-child) {
    aspect-ratio: 2 / 3;
  }
  @media (max-width: 520px) {
    .covers {
      grid-template-columns: repeat(3, minmax(0, 1fr));
      gap: var(--space-4) var(--space-3);
    }
  }
  .load-error {
    align-items: center;
    justify-content: space-between;
    flex-wrap: wrap;
  }
</style>
