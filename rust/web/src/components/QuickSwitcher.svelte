<script lang="ts">
  // The command palette (Ctrl+K or ⌘K, and Search in the bar): one search over pages, host actions,
  // diagnostics, games, devices and settings, on the console's Dialog. A combobox: the field keeps focus,
  // the arrows move through the results, Enter opens or runs one. Games and devices load each time it
  // opens; nothing polls. What it lists is quick-switch-70.ts's.
  import { onDestroy, tick, untrack } from 'svelte';
  import Dialog from './Dialog.svelte';
  import Icon from './Icon.svelte';
  import AppCover from './library/AppCover.svelte';
  import { backAfterRestart, confirmRestart } from './maintenance/host';
  import { api, type App, type Client } from '../lib/api';
  import { confirm, failed, notify } from '../lib/feedback.svelte';
  import type { ActionId, Entry } from '../lib/quick-switch';
  import { enterVerb, palette } from '../lib/quick-switch-70';
  import { navigate, route } from '../lib/router.svelte';
  import { refreshLive, session } from '../lib/session.svelte';
  import type { Theme } from '../lib/theme';

  let {
    open = $bindable(false),
    theme = 'system',
    ontheme,
    onlibrarychanged,
  }: {
    open?: boolean;
    /** The console's theme: the palette offers the others. */
    theme?: Theme;
    ontheme?: (theme: Theme) => void;
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

  const groups = $derived(palette(query, { apps, clients }, { theme, streams: session.live?.streams ?? 0 }));
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
      else notify('Rubylight is taking longer than usual to restart. Refresh if it does not come back.', 'danger');
    } else if (action === 'sync') {
      await sync();
    } else if (action === 'stop') {
      await stopStreams();
    } else if (action === 'theme-light' || action === 'theme-dark' || action === 'theme-system') {
      ontheme?.(action === 'theme-light' ? 'light' : action === 'theme-dark' ? 'dark' : 'system');
    }
  }

  /** Disconnects every device that streams, after one question. The running game stays open. */
  async function stopStreams() {
    let streams;
    try {
      streams = (await api.sessions.streams()).sessions;
    } catch (error) {
      failed('Reading the streams failed', error);
      return;
    }
    if (!streams.length) {
      notify('No device is streaming.');
      return;
    }
    const names = streams.map((stream) => stream.device_name || 'a device').join(', ');
    const ok = await confirm({
      title: streams.length === 1 ? `Disconnect ${names}?` : `Disconnect ${streams.length} devices?`,
      message: `${streams.length === 1 ? 'Every connection from this device ends.' : `${names}: every connection from them ends.`} The running app stays open, and a device can connect again.`,
      confirm: 'Disconnect',
    });
    if (!ok) return;
    const failures = (await Promise.allSettled(streams.map((stream) => api.clients.disconnect(stream.uuid)))).filter((result) => result.status === 'rejected');
    if (failures.length) failed('Disconnecting failed', (failures[0] as PromiseRejectedResult).reason);
    else notify(streams.length === 1 ? `Disconnected ${names}.` : `Disconnected ${streams.length} devices.`, 'ok');
    await refreshLive();
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
        placeholder="Games, devices, settings"
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
                    {#if entry.detail}<span class="detail">{#if entry.status}<span class="dot {entry.status}" aria-hidden="true"></span>{/if}{entry.detail}</span>{/if}
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
      <span><kbd>Enter</kbd> {enterVerb(entries[active])}</span>
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
    letter-spacing: var(--tracking-label);
    text-transform: uppercase;
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
    width: 32px;
    color: var(--muted);
  }
  .active .lead {
    color: var(--accent-text);
  }
  .thumb {
    display: block;
    width: 28px;
    border-radius: var(--radius-sm);
    box-shadow: var(--edge);
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
    display: inline-flex;
    align-items: center;
    gap: var(--space-1);
    font-size: var(--text-xs);
    color: var(--muted);
    overflow-wrap: anywhere;
  }
  /* A device's state: filled while it is connected, hollow while it is away. */
  .dot {
    flex: none;
    width: 8px;
    height: 8px;
    border: 1px solid var(--muted);
    border-radius: 50%;
  }
  .dot.connected {
    border-color: var(--ok);
    background: var(--ok);
  }
  .dot.disabled {
    border-style: dashed;
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
