<script lang="ts">
  // The quick switcher (Ctrl+K or ⌘K, and Search in the sidebar): one search over pages, actions, games,
  // devices and settings, on the console's Dialog. A combobox: the field keeps focus, the arrows move
  // through the results, Enter opens one. Games and devices load each time it opens; nothing polls.
  import { onDestroy, tick, untrack } from 'svelte';
  import Dialog from './Dialog.svelte';
  import Icon from './Icon.svelte';
  import AppCover from './library/AppCover.svelte';
  import { backAfterRestart, confirmRestart } from './maintenance/host';
  import { api, type App, type Client } from '../lib/api';
  import { failed, notify } from '../lib/feedback.svelte';
  import { results, type ActionId, type Entry } from '../lib/quick-switch';
  import { navigate, route } from '../lib/router.svelte';

  let {
    open = $bindable(false),
    onlibrarychanged,
  }: {
    open?: boolean;
    /** A sync from here changed what the Games page lists. */
    onlibrarychanged?: () => void;
  } = $props();

  const id = $props.id();
  let query = $state('');
  let apps = $state<App[] | null>(null);
  let clients = $state<Client[] | null>(null);
  let active = $state(0);
  let input = $state<HTMLInputElement>();
  let list = $state<HTMLElement>();

  const groups = $derived(results(query, { apps, clients }));
  const entries = $derived(groups.flatMap((group) => group.entries));
  const optionId = (at: number) => `${id}-option-${at}`;

  // Each time it opens: an empty field, the first result marked, and fresh games and devices.
  $effect(() => {
    if (!open) return;
    untrack(() => {
      query = '';
      active = 0;
      void load();
      void tick().then(() => input?.focus());
    });
  });

  async function load() {
    await Promise.all([
      api.apps.list().then((library) => (apps = library.apps ?? []), () => {}),
      api.clients.list().then((found) => (clients = found.clients ?? []), () => {}),
    ]);
  }

  function typed() {
    active = 0;
    if (list) list.scrollTop = 0;
  }

  function move(to: number) {
    if (!entries.length) return;
    active = (to + entries.length) % entries.length;
    void tick().then(() => document.getElementById(optionId(active))?.scrollIntoView({ block: 'nearest' }));
  }

  /**
   * Focus stays in the field (aria-activedescendant), but a screen reader can still focus an
   * option itself: Enter or Space there chooses it.
   */
  function optionKey(event: KeyboardEvent, entry: Entry) {
    if (event.key !== 'Enter' && event.key !== ' ') return;
    event.preventDefault();
    void choose(entry);
  }

  function keydown(event: KeyboardEvent) {
    if (event.isComposing) return;
    if (event.key === 'ArrowDown') move(active + 1);
    else if (event.key === 'ArrowUp') move(active - 1);
    else if (event.key === 'PageDown') move(Math.min(active + 5, entries.length - 1));
    else if (event.key === 'PageUp') move(Math.max(active - 5, 0));
    else if (event.key === 'Enter') {
      const entry = entries[active];
      if (entry) void choose(entry);
    } else return;
    event.preventDefault();
  }

  async function choose(entry: Entry) {
    open = false;
    // The dialog closes and hands focus back before the page changes.
    await tick();
    if (entry.href) {
      navigate(entry.href);
      if (entry.reveal) reveal(entry.reveal);
    } else if (entry.action) {
      await run(entry.action);
    }
  }

  async function run(action: ActionId) {
    if (action === 'bundle') {
      const download = document.createElement('a');
      download.href = api.logs.supportBundleUrl;
      download.download = '';
      download.click();
    } else if (action === 'restart') {
      if (!(await confirmRestart())) return;
      try {
        await api.host.restart();
      } catch (error) {
        failed('Restart failed', error);
        return;
      }
      notify('Rubylight is restarting.');
      if (await backAfterRestart()) notify('Rubylight restarted.', 'ok');
    } else if (action === 'sync') {
      await sync();
    }
  }

  /** The Games page's Sync now, from anywhere: every launcher that is on, and Playnite when it runs. */
  async function sync() {
    notify('Syncing your games with the launchers…');
    const playnite = await api.playnite.status().catch(() => null);
    const played = playnite?.active
      ? api.playnite.sync().catch((error: unknown) => {
          failed('Syncing Playnite failed', error);
          return null;
        })
      : Promise.resolve(null);
    try {
      const [steam, launchers] = await Promise.all([api.steam.sync().catch(() => null), api.library.sync()]);
      const changed = (steam?.changed ?? false) || launchers.added + launchers.removed > 0 || ((await played)?.changed ?? false);
      notify(changed ? 'Your library is up to date with your launchers.' : 'Nothing new: your library already has every game.', 'ok');
      onlibrarychanged?.();
    } catch (error) {
      await played;
      failed('Syncing the launchers failed', error);
    }
  }

  /** Brings a device's row on the Devices page into view, marks it and focuses its first button. */
  // One pending look at a time: a newer reveal, or closing the app shell, cancels
  // it, so a stale search cannot take focus seconds later.
  let revealTimer: ReturnType<typeof setTimeout> | undefined;
  onDestroy(() => clearTimeout(revealTimer));
  function reveal(name: string) {
    clearTimeout(revealTimer);
    const until = Date.now() + 3000;
    const look = () => {
      revealTimer = undefined;
      // The page changes a frame later during a view transition, and its devices load after that.
      const row =
        route.path === '/devices'
          ? [...document.querySelectorAll<HTMLElement>('main li')].find((item) => item.querySelector('.name')?.textContent?.trim() === name)
          : undefined;
      if (!row) {
        if (Date.now() < until) revealTimer = setTimeout(look, 100);
        return;
      }
      row.scrollIntoView({ block: 'center' });
      row.querySelector<HTMLElement>('button')?.focus({ preventScroll: true });
      const mark = getComputedStyle(row).getPropertyValue('--accent-soft').trim();
      if (mark && !matchMedia('(prefers-reduced-motion: reduce)').matches) {
        row.animate([{ backgroundColor: mark }, { backgroundColor: 'transparent' }], { duration: 1800, easing: 'ease-out' });
      }
    };
    revealTimer = setTimeout(look, 0);
  }
</script>

<Dialog bind:open title="Search" size="md">
  <div class="switcher">
    <div class="field">
      <span class="search-icon"><Icon name="search" size={18} /></span>
      <input
        bind:this={input}
        class="input"
        type="text"
        role="combobox"
        aria-label="Search games, devices, settings and actions"
        aria-expanded={entries.length > 0}
        aria-controls="{id}-results"
        aria-autocomplete="list"
        aria-activedescendant={entries.length ? optionId(active) : undefined}
        placeholder="Games, devices, settings, actions"
        autocomplete="off"
        spellcheck="false"
        bind:value={query}
        oninput={typed}
        onkeydown={keydown}
      />
    </div>
    <div class="results" bind:this={list}>
      {#if entries.length}
        <div id="{id}-results" role="listbox" aria-label="Results">
          {#each groups as group (group.id)}
            <div class="group" role="group" aria-labelledby="{id}-{group.id}">
              <div class="group-title" id="{id}-{group.id}">{group.title}</div>
              {#each group.entries as entry (entry.id)}
                {@const at = entries.indexOf(entry)}
                <div
                  class="option"
                  class:active={at === active}
                  id={optionId(at)}
                  role="option"
                  tabindex="-1"
                  aria-selected={at === active}
                  onpointermove={() => (active = at)}
                  onmousedown={(event) => event.preventDefault()}
                  onclick={() => choose(entry)}
                  onkeydown={(event) => optionKey(event, entry)}
                >
                  <span class="lead" aria-hidden="true">
                    {#if entry.cover}
                      <span class="thumb"><AppCover name={entry.label} uuid={entry.cover.uuid} path={entry.cover.path} /></span>
                    {:else}
                      <Icon name={entry.icon} />
                    {/if}
                  </span>
                  <span class="text">
                    <span class="label">{entry.label}</span>
                    {#if entry.detail}<span class="detail">{entry.detail}</span>{/if}
                  </span>
                  {#if at === active}<kbd class="enter" aria-hidden="true">Enter</kbd>{/if}
                </div>
              {/each}
            </div>
          {/each}
        </div>
      {:else}
        <p class="none" id="{id}-results">Nothing matches “{query.trim()}”.</p>
      {/if}
    </div>
    <p class="visually-hidden" role="status">
      {entries.length === 0 ? 'No results' : entries.length === 1 ? '1 result' : `${entries.length} results`}
    </p>
    <div class="keys" aria-hidden="true">
      <span><kbd>↑</kbd><kbd>↓</kbd> Move</span>
      <span><kbd>Enter</kbd> Open</span>
      <span><kbd>Esc</kbd> Close</span>
    </div>
  </div>
</Dialog>

<style>
  .switcher {
    display: grid;
    gap: var(--space-3);
    min-width: 0;
  }
  .field {
    position: relative;
  }
  .search-icon {
    position: absolute;
    left: var(--space-3);
    top: 50%;
    transform: translateY(-50%);
    color: var(--muted);
    pointer-events: none;
  }
  .field .input {
    padding-left: 40px;
  }
  /* A steady height: the dialog doesn't jump as the results change. */
  .results {
    height: min(380px, 52dvh);
    overflow-y: auto;
    overscroll-behavior: contain;
    margin: 0 calc(var(--space-2) * -1);
    padding: 0 var(--space-2);
  }
  .group + .group {
    margin-top: var(--space-2);
  }
  .group-title {
    padding: var(--space-2) var(--space-3) var(--space-1);
    font-size: var(--text-xs);
    font-weight: var(--weight-semibold);
    color: var(--muted);
  }
  .option {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    min-height: var(--control-height);
    padding: var(--space-1) var(--space-3);
    border-radius: var(--radius);
    color: var(--ink-2);
    cursor: pointer;
  }
  /* One mark for the pointer and the arrows, the sidebar's current-page look. */
  .option.active {
    background: var(--accent-soft);
    color: var(--ink);
  }
  .lead {
    flex: none;
    display: grid;
    place-items: center;
    width: 28px;
    color: var(--muted);
  }
  .active .lead {
    color: var(--accent-text);
  }
  .thumb {
    display: block;
    width: 24px;
  }
  .thumb :global(.cover) {
    border-radius: var(--radius-sm);
  }
  .thumb :global(.mark) {
    font-size: 9px;
  }
  .text {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 0 var(--space-2);
    min-width: 0;
    flex: 1 1 auto;
  }
  .label {
    font-weight: var(--weight-medium);
    overflow-wrap: anywhere;
  }
  .detail {
    font-size: var(--text-xs);
    color: var(--muted);
    overflow-wrap: anywhere;
  }
  .enter {
    flex: none;
  }
  kbd {
    display: inline-block;
    min-width: 1.6em;
    padding: 1px 5px;
    border: 1px solid var(--line);
    border-radius: var(--radius-sm);
    background: var(--sunken);
    color: var(--muted);
    font-size: var(--text-xs);
    line-height: 1.4;
    text-align: center;
  }
  .none {
    padding: var(--space-4) var(--space-3);
    font-size: var(--text-sm);
    color: var(--muted);
  }
  .keys {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-2) var(--space-4);
    padding-top: var(--space-3);
    border-top: 1px solid var(--line);
    font-size: var(--text-xs);
    color: var(--muted);
  }
  .keys kbd + kbd {
    margin-left: 2px;
  }
  /* A touch screen has no arrows to press. */
  @media (hover: none) {
    .keys,
    .enter {
      display: none;
    }
  }
</style>
