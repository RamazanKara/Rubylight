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
    if (canReorder) return 'Drag a cover to reorder, or open a game to move it.';
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
      <span class="push">
      <Button size="sm" icon="check" disabled={busy !== '' || !apps.some((app) => app.uuid)} onclick={() => selection.start()}>Select</Button>
      </span>
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
    <span class="count">{needle ? `${shown.length} of ${apps.length}` : apps.length} {apps.length === 1 ? 'app' : 'apps'}</span>
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
              <button type="button" class="fab stop" title="Close" aria-busy={busy === 'close'} disabled={busy === 'close'} onclick={close}>
                {#if busy === 'close'}<span class="spinner" aria-hidden="true"></span>{:else}<Icon name="stop" size={18} />{/if}
                <span class="visually-hidden">Close {app.name}</span>
              </button>
            {:else if notInstalled(app)}
              <button
                type="button"
                class="fab"
                title="Remove"
                aria-busy={busy === `remove:${app.uuid}`}
                disabled={busy !== ''}
                onclick={() => remove(app)}
              >
                {#if busy === `remove:${app.uuid}`}<span class="spinner" aria-hidden="true"></span>{:else}<Icon name="trash" size={18} />{/if}
                <span class="visually-hidden">Remove {app.name}</span>
              </button>
            {:else if app.uuid}
              <button
                type="button"
                class="fab launch reveal"
                class:busy={busy === `launch:${app.uuid}`}
                title={running ? `Close ${running} first` : 'Launch on PC'}
                aria-busy={busy === `launch:${app.uuid}`}
                disabled={running !== '' || busy !== ''}
                onclick={() => launch(app)}
              >
                {#if busy === `launch:${app.uuid}`}<span class="spinner" aria-hidden="true"></span>{:else}<Icon name="play" size={18} />{/if}
                <span class="visually-hidden">Launch {app.name} on PC</span>
              </button>
            {/if}
          </div>
        </li>
      {/each}
    </ul>
  {/if}
</div>

<style>
  .library {
    display: grid;
    gap: var(--space-3);
  }
  .toolbar {
    display: flex;
    gap: var(--space-2) var(--space-4);
    flex-wrap: wrap;
    align-items: center;
  }
  .search {
    position: relative;
    flex: 1 1 240px;
    max-width: 380px;
  }
  .search-icon {
    position: absolute;
    left: var(--space-3);
    top: 50%;
    transform: translateY(-50%);
    color: var(--muted);
    pointer-events: none;
  }
  .search .input {
    padding-left: 38px;
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
  .selectbar {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-2);
    padding: var(--space-2) var(--space-2) var(--space-2) var(--space-4);
    border: 1px solid var(--line);
    border-radius: var(--radius-lg);
    background: var(--panel);
    box-shadow: var(--shadow);
  }
  .selectbar .select {
    width: auto;
  }
  .picked-count {
    font-size: var(--text-sm);
    font-weight: var(--weight-semibold);
    margin-right: var(--space-2);
  }
  .push {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-2);
    margin-left: auto;
  }
  .summary {
    display: flex;
    flex-wrap: wrap;
    gap: 0 6px;
    font-size: var(--text-xs);
  }
  .count {
    color: var(--ink-2);
    font-weight: var(--weight-medium);
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
    grid-template-columns: repeat(auto-fill, minmax(144px, 1fr));
    gap: var(--space-5) var(--space-4);
    margin: var(--space-1) 0 0;
    padding: 0;
    list-style: none;
  }
  /* Three across on a phone: covers stay recognisable and the list stays short. */
  @media (max-width: 520px) {
    .grid {
      grid-template-columns: repeat(3, minmax(0, 1fr));
      gap: var(--space-4) var(--space-3);
    }
  }
  .card {
    position: relative;
    display: grid;
    gap: var(--space-2);
    align-content: start;
    min-width: 0;
  }
  .card[draggable='true'] {
    cursor: grab;
  }
  .dragging .art {
    opacity: 0.45;
    outline: 2px dashed var(--line-strong);
    outline-offset: 2px;
  }
  .open {
    display: grid;
    gap: var(--space-2);
    min-width: 0;
    color: inherit;
    text-decoration: none;
    border-radius: var(--radius);
  }
  .art {
    position: relative;
    display: block;
    border-radius: var(--radius);
    transition:
      transform var(--duration-fast) var(--ease-standard),
      box-shadow var(--duration-fast) var(--ease-standard);
  }
  a.open:hover .art,
  .pick:hover .art {
    transform: translateY(-2px);
    box-shadow: var(--shadow-raised);
  }
  a.open:hover .name {
    color: var(--accent-text);
  }
  .pick {
    cursor: pointer;
  }
  .picked .art,
  .picked:hover .art {
    outline: 3px solid var(--accent);
    outline-offset: 2px;
  }
  .tick {
    position: absolute;
    top: var(--space-2);
    right: var(--space-2);
    width: 22px;
    height: 22px;
    margin: 0;
    accent-color: var(--accent);
    cursor: pointer;
  }
  .state {
    position: absolute;
    top: var(--space-2);
    left: var(--space-2);
  }
  .name {
    font-weight: var(--weight-medium);
    font-size: var(--text-sm);
    line-height: var(--leading-snug);
    overflow-wrap: anywhere;
    display: -webkit-box;
    -webkit-box-orient: vertical;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    overflow: hidden;
    transition: color var(--duration-fast) var(--ease-standard);
  }
  .note {
    font-size: var(--text-xs);
  }
  /* The card's one action sits on its cover's lower corner. A percentage margin
     counts from the card's width, so 150% is the 2:3 cover's height. */
  .actions {
    position: absolute;
    top: 0;
    right: var(--space-2);
    margin-top: calc(150% - var(--control-height) - var(--space-2));
    display: flex;
  }
  .actions:empty {
    display: none;
  }
  .fab {
    display: grid;
    place-items: center;
    width: var(--control-height);
    height: var(--control-height);
    padding: 0;
    border: 1px solid var(--media-line);
    border-radius: var(--radius-pill);
    background: var(--media-bg);
    color: var(--media-ink);
    box-shadow: var(--shadow-raised);
    cursor: pointer;
    transition:
      opacity var(--duration-fast) var(--ease-standard),
      transform var(--duration-fast) var(--ease-standard),
      background-color var(--duration-fast) var(--ease-standard);
  }
  .fab:hover:not(:disabled) {
    transform: scale(1.06);
  }
  /* Solid play and stop shapes read better than outlines at this size. */
  .launch :global(path),
  .stop :global(path) {
    fill: currentColor;
  }
  .fab.launch:hover:not(:disabled),
  .fab.launch:focus-visible {
    background: var(--accent);
    border-color: var(--accent);
    color: var(--accent-ink);
  }
  .fab:disabled {
    cursor: not-allowed;
    color: var(--media-idle);
  }
  .fab.busy:disabled {
    color: var(--media-ink);
  }
  /* With a mouse, Launch shows when the card is pointed at or focused; on touch it
     always shows. Hidden, it takes no taps; a touch laptop (any coarse pointer)
     keeps it visible. */
  @media (hover: hover) and (pointer: fine) and (not (any-pointer: coarse)) {
    .reveal {
      opacity: 0;
      transform: translateY(4px);
      pointer-events: none;
    }
    .card:hover .reveal,
    .card:focus-within .reveal,
    .reveal.busy {
      opacity: 1;
      transform: none;
      pointer-events: auto;
    }
    .card:hover .reveal:disabled:not(.busy) {
      opacity: 0.7;
    }
  }
  .spinner {
    width: 16px;
    height: 16px;
    border: 2px solid currentColor;
    border-right-color: transparent;
    border-radius: var(--radius-pill);
    animation: spin 0.8s linear infinite;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .spinner {
      animation: none;
      border-right-color: currentColor;
      opacity: 0.6;
    }
  }
</style>
