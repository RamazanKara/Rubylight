<script lang="ts">
  import Badge from '../Badge.svelte';
  import Button from '../Button.svelte';
  import Icon from '../Icon.svelte';
  import AppCover from './AppCover.svelte';
  import { api, type App, type SessionStatus } from '../../lib/api';
  import { confirm, failed, notify } from '../../lib/feedback.svelte';
  import { link } from '../../lib/router.svelte';
  import { closeApp, confirmClose, coverPath, launchApp, runningName } from './app';
  import { AppSelection, counted, launcherCounts, type Launcher } from './selection.svelte';

  let {
    apps,
    status,
    reordering,
    onreorder,
    onstatus,
    onremoved,
  }: {
    apps: App[];
    status: SessionStatus | null;
    reordering: boolean;
    /** The whole list in its new order. */
    onreorder: (next: App[]) => void;
    /** An app was started or closed. */
    onstatus: () => void;
    /** Apps were removed: one whose game is not installed, or the selected ones. */
    onremoved?: () => void;
  } = $props();

  const uid = $props.id();
  let query = $state('');
  let sort = $state<'order' | 'name'>('order');
  /** `launch:uuid`, `remove:uuid`, `remove-selected` or `close` while a request is out. */
  let busy = $state('');
  /** Select mode: a card picks its app for Remove instead of opening it. */
  const selection = new AppSelection();
  const selecting = $derived(selection.selecting);

  // While a card is dragged the grid shows this order; dropping saves it.
  let dragging = $state<string | null>(null);
  let preview = $state.raw<App[] | null>(null);

  const running = $derived(runningName(status));
  const needle = $derived(query.trim().toLowerCase());
  const missingIds = $derived(apps.some((app) => !app.uuid));
  const canReorder = $derived(!selecting && !reordering && sort === 'order' && !needle && !missingIds && apps.length > 1);
  const chosen = $derived(selection.chosen(apps));
  const launchers = $derived(launcherCounts(apps));

  const shown = $derived.by(() => {
    let list = preview ?? apps;
    if (needle) list = list.filter((app) => app.name.toLowerCase().includes(needle));
    if (sort === 'name') {
      list = [...list].sort((a, b) => a.name.localeCompare(b.name, undefined, { numeric: true, sensitivity: 'base' }));
    }
    return list;
  });

  const hint = $derived.by(() => {
    if (selecting) return 'Select the apps to remove, one by one or by launcher.';
    if (reordering) return 'Saving the library order…';
    if (apps.length < 2) return '';
    if (missingIds) return 'Restart the host to enable reordering for all apps.';
    if (canReorder) return 'Drag a cover to reorder, or open an app and choose Move earlier or Move later.';
    return 'Clear the search and sort by library order to drag apps into place.';
  });

  async function launch(app: App) {
    busy = `launch:${app.uuid}`;
    await launchApp(app);
    busy = '';
    onstatus();
  }

  // The host marks an app `not-installed` when its launcher was read and the game is gone.
  const notInstalled = (app: App) => app['not-installed'] === true;

  async function remove(app: App) {
    if (!app.uuid) return;
    const name = app.name || 'this app';
    const ok = await confirm({
      title: `Remove ${name}?`,
      message: 'Its game is not installed on this PC. It leaves the library, and no sync adds it back.',
      confirm: 'Remove',
      danger: true,
    });
    if (!ok) return;
    busy = `remove:${app.uuid}`;
    try {
      await api.apps.remove(app.uuid);
      notify(`Removed ${name}.`, 'ok');
      onremoved?.();
    } catch (error) {
      failed('Removing the app failed', error);
    } finally {
      busy = '';
    }
  }

  async function removeSelected() {
    const list = chosen;
    if (list.length === 0) return;
    const what = list.length === 1 ? list[0]?.name || 'this app' : counted(list.length, 'app');
    const ok = await confirm({
      title: `Remove ${what}?`,
      message: `${list.length === 1 ? 'It leaves the library, and no sync adds it back' : 'They leave the library, and no sync adds them back'}. A removed game can be restored under Where your games come from.`,
      confirm: 'Remove',
      danger: true,
    });
    if (!ok) return;
    busy = 'remove-selected';
    try {
      const message = await selection.remove(apps, api.apps.removeMany);
      notify(message.text, message.tone);
    } catch (error) {
      failed('Removing the apps failed', error);
    } finally {
      busy = '';
      // Also after a failure: the host may have removed some before it gave up.
      onremoved?.();
    }
  }

  async function close() {
    const name = running;
    if (!(await confirmClose(name))) return;
    busy = 'close';
    await closeApp(name);
    busy = '';
    onstatus();
  }

  function sortable(node: HTMLElement) {
    const uuidAt = (event: DragEvent) =>
      event.target instanceof Element ? event.target.closest<HTMLElement>('[data-uuid]')?.dataset.uuid : undefined;
    const finish = () => {
      dragging = null;
      preview = null;
    };
    const start = (event: DragEvent) => {
      const uuid = uuidAt(event);
      if (!canReorder || !uuid) {
        event.preventDefault();
        return;
      }
      dragging = uuid;
      preview = apps.slice();
      if (event.dataTransfer) {
        event.dataTransfer.effectAllowed = 'move';
        // Firefox starts no drag without data.
        event.dataTransfer.setData('text/plain', uuid);
      }
    };
    const over = (event: DragEvent) => {
      if (!dragging || !preview) return;
      event.preventDefault();
      if (event.dataTransfer) event.dataTransfer.dropEffect = 'move';
      const target = uuidAt(event);
      if (!target || target === dragging) return;
      const from = preview.findIndex((app) => app.uuid === dragging);
      const to = preview.findIndex((app) => app.uuid === target);
      if (from < 0 || to < 0) return;
      // The dragged card takes the slot under the pointer.
      const next = preview.slice();
      const [moved] = next.splice(from, 1);
      if (moved) next.splice(to, 0, moved);
      preview = next;
    };
    const drop = (event: DragEvent) => {
      if (!dragging || !preview) return;
      event.preventDefault();
      const next = preview;
      finish();
      if (next.some((app, index) => app !== apps[index])) onreorder(next);
    };
    node.addEventListener('dragstart', start);
    node.addEventListener('dragover', over);
    node.addEventListener('drop', drop);
    node.addEventListener('dragend', finish);
    return () => {
      node.removeEventListener('dragstart', start);
      node.removeEventListener('dragover', over);
      node.removeEventListener('drop', drop);
      node.removeEventListener('dragend', finish);
    };
  }
</script>

<div class="library">
  <div class="toolbar">
    <div class="search">
      <label class="visually-hidden" for="{uid}-search">Search apps by name</label>
      <span class="search-icon"><Icon name="search" size={16} /></span>
      <input
        id="{uid}-search"
        class="input"
        type="search"
        placeholder="Search by name"
        autocomplete="off"
        spellcheck="false"
        bind:value={query}
      />
    </div>
    <div class="sort">
      <label for="{uid}-sort">Sort</label>
      <select id="{uid}-sort" class="select" bind:value={sort}>
        <option value="order">Library order</option>
        <option value="name">Name</option>
      </select>
    </div>
    {#if !selecting}
      <Button size="sm" icon="check" disabled={busy !== '' || !apps.some((app) => app.uuid)} onclick={() => selection.start()}>Select</Button>
    {/if}
  </div>
  {#if selecting}
    <div class="selectbar" role="group" aria-label="Selected apps">
      <span class="picked-count" role="status">{chosen.length} selected</span>
      <Button size="sm" variant="ghost" onclick={() => selection.add(shown)}>Select all</Button>
      <Button size="sm" variant="ghost" disabled={chosen.length === 0} onclick={() => selection.clear()}>Select none</Button>
      {#if launchers.length > 0}
        <label class="visually-hidden" for="{uid}-launcher">Select every app of a launcher</label>
        <select
          id="{uid}-launcher"
          class="select"
          value=""
          onchange={(event) => {
            selection.addLauncher(apps, event.currentTarget.value as Launcher);
            event.currentTarget.value = '';
          }}
        >
          <option value="">Select by launcher</option>
          {#each launchers as entry (entry.launcher)}
            <option value={entry.launcher}>{entry.name} ({entry.count})</option>
          {/each}
        </select>
      {/if}
      <span class="push">
        <Button
          size="sm"
          variant="danger"
          icon="trash"
          busy={busy === 'remove-selected'}
          disabled={chosen.length === 0 || busy !== ''}
          onclick={removeSelected}
        >Remove{chosen.length > 0 ? ` ${counted(chosen.length, 'app')}` : ''}</Button>
        <Button size="sm" disabled={busy === 'remove-selected'} onclick={() => selection.stop()}>Done</Button>
      </span>
    </div>
  {/if}
  <p class="muted summary" role="status">
    <span>{needle ? `${shown.length} of ${apps.length}` : apps.length} {apps.length === 1 ? 'app' : 'apps'}</span>
    {#if hint}<span class="dot" aria-hidden="true">·</span> <span>{hint}</span>{/if}
  </p>

  {#if shown.length === 0}
    <p class="muted none">No apps match “{query.trim()}”.</p>
  {:else}
    <ul class="grid" aria-label="Apps" {@attach sortable}>
      {#each shown as app (app)}
        {@const isRunning = running !== '' && app.name === running}
        <li
          class="card"
          class:dragging={dragging !== null && app.uuid === dragging}
          data-uuid={app.uuid}
          draggable={canReorder && !!app.uuid}
        >
          {#if selecting && app.uuid}
            {@const uuid = app.uuid}
            <label class="open pick" class:picked={selection.selected.has(uuid)}>
              <span class="art">
                <AppCover name={app.name} {uuid} path={coverPath(app)} />
                <input class="tick" type="checkbox" checked={selection.selected.has(uuid)} onchange={() => selection.toggle(uuid)} />
              </span>
              <span class="name">{app.name || 'Unnamed app'}</span>
            </label>
          {:else if app.uuid}
            <a class="open" href="/library/{encodeURIComponent(app.uuid)}" use:link draggable="false">
              <div class="art">
                <AppCover name={app.name} uuid={app.uuid} path={coverPath(app)} />
                {#if isRunning}
                  <span class="state">
                    {#if status?.paused}<Badge tone="warn">Paused</Badge>{:else}<Badge tone="live">Running</Badge>{/if}
                  </span>
                {:else if notInstalled(app)}
                  <span class="state"><Badge>Not installed</Badge></span>
                {/if}
              </div>
              <span class="name">{app.name || 'Unnamed app'}</span>
            </a>
          {:else}
            <div class="open" title="Restart the host to edit this app.">
              <div class="art"><AppCover name={app.name} /></div>
              <span class="name">{app.name || 'Unnamed app'}</span>
              <span class="muted note">Restart the host to edit</span>
            </div>
          {/if}
          <div class="actions">
            {#if selecting}
              <!-- Select mode only picks apps; Launch and Close come back with Done. -->
            {:else if isRunning}
              <Button size="sm" icon="stop" busy={busy === 'close'} onclick={close}>
                Close<span class="visually-hidden"> {app.name}</span>
              </Button>
            {:else if notInstalled(app)}
              <Button size="sm" icon="trash" busy={busy === `remove:${app.uuid}`} disabled={busy !== ''} onclick={() => remove(app)}>
                Remove<span class="visually-hidden"> {app.name}</span>
              </Button>
            {:else}
              <Button
                size="sm"
                icon="play"
                disabled={!app.uuid || running !== '' || busy !== ''}
                busy={busy === `launch:${app.uuid}`}
                title={running ? `Close ${running} first` : undefined}
                onclick={() => launch(app)}
              >Launch on PC<span class="visually-hidden"> {app.name}</span>
              </Button>
            {/if}
          </div>
        </li>
      {/each}
    </ul>
  {/if}
</div>

<style>
  .toolbar {
    display: flex;
    gap: var(--space-2) var(--space-3);
    flex-wrap: wrap;
    align-items: center;
  }
  .search {
    position: relative;
    flex: 1 1 220px;
    max-width: 360px;
  }
  .search-icon {
    position: absolute;
    left: 10px;
    top: 50%;
    transform: translateY(-50%);
    color: var(--muted);
    pointer-events: none;
  }
  .search .input {
    padding-left: var(--space-6);
  }
  .sort {
    display: flex;
    align-items: center;
    gap: var(--space-2);
  }
  .sort label {
    font-size: var(--text-sm);
    color: var(--muted);
  }
  .sort .select {
    width: auto;
  }
  .library {
    display: grid;
    gap: var(--space-3);
  }
  .selectbar {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-2);
  }
  .selectbar .select {
    width: auto;
  }
  .picked-count {
    font-size: var(--text-sm);
    font-weight: var(--weight-medium);
    margin-right: var(--space-2);
  }
  .push {
    display: flex;
    gap: var(--space-2);
    margin-left: auto;
  }
  .summary {
    display: flex;
    flex-wrap: wrap;
    gap: 0 6px;
    font-size: var(--text-xs);
  }
  /* On a phone the hint gets its own line rather than a separator left hanging. */
  @media (max-width: 520px) {
    .summary {
      flex-direction: column;
    }
    .dot {
      display: none;
    }
  }
  .none {
    font-size: var(--text-sm);
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(136px, 1fr));
    gap: var(--space-5) var(--space-4);
    margin: var(--space-2) 0 0;
    padding: 0;
    list-style: none;
  }
  .card {
    display: grid;
    gap: var(--space-2);
    align-content: start;
    min-width: 0;
  }
  .card[draggable='true'] {
    cursor: grab;
  }
  .dragging {
    opacity: 0.4;
  }
  .open {
    display: grid;
    gap: 6px;
    min-width: 0;
    color: inherit;
    text-decoration: none;
    border-radius: var(--radius);
  }
  .art {
    position: relative;
    border-radius: var(--radius);
  }
  a.open:hover .art {
    box-shadow: 0 0 0 1px var(--muted);
  }
  .pick {
    cursor: pointer;
  }
  .pick .art {
    display: block;
  }
  .pick:hover .art {
    box-shadow: 0 0 0 1px var(--muted);
  }
  .picked .art,
  .picked:hover .art {
    box-shadow: 0 0 0 2px var(--accent);
  }
  .tick {
    position: absolute;
    top: 6px;
    right: 6px;
    width: 20px;
    height: 20px;
    margin: 0;
    accent-color: var(--accent);
    cursor: pointer;
  }
  a.open:hover .name {
    text-decoration: underline;
    text-underline-offset: 2px;
  }
  .state {
    position: absolute;
    top: 6px;
    left: 6px;
  }
  .name {
    font-weight: var(--weight-medium);
    font-size: var(--text-sm);
    line-height: 1.35;
    overflow-wrap: anywhere;
    display: -webkit-box;
    -webkit-box-orient: vertical;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    overflow: hidden;
  }
  .note {
    font-size: var(--text-xs);
  }
  .actions {
    display: flex;
  }
</style>
