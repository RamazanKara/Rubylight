<script lang="ts">
  import { onMount, tick } from 'svelte';
  import Button from '../components/Button.svelte';
  import Disclosure from '../components/Disclosure.svelte';
  import EmptyState from '../components/EmptyState.svelte';
  import Icon from '../components/Icon.svelte';
  import PageHeader from '../components/PageHeader.svelte';
  import Panel from '../components/Panel.svelte';
  import SettingRow from '../components/settings/SettingRow.svelte';
  import Switch from '../components/settings/Switch.svelte';
  import UnknownKeys from '../components/settings/UnknownKeys.svelte';
  import LimiterPanel from '../components/maintenance/LimiterPanel.svelte';
  import PasswordPanel from '../components/maintenance/PasswordPanel.svelte';
  import PowerPanel from '../components/maintenance/PowerPanel.svelte';
  import SessionsPanel from '../components/maintenance/SessionsPanel.svelte';
  import UpdatesPanel from '../components/maintenance/UpdatesPanel.svelte';
  import VulkanPanel from '../components/maintenance/VulkanPanel.svelte';
  import InternetPlayPanel from '../components/maintenance/InternetPlayPanel.svelte';
  import { reason } from '../components/maintenance/util';
  import { matchesSetting, matchesWords, searchWords } from '../components/settings/search';
  import {
    applyChanges,
    caution,
    computeChanges,
    currentValues,
    differsFromDefault,
    foldStatus,
    isSet,
    effective,
    has,
    nativeEncoders,
    normalise,
    NOT_SETTINGS,
    nvidiaEnhancements,
    sameValue,
    settingsOnly,
    type Edits,
  } from '../components/settings/values';
  import { api, type Config, type ConfigValue, type WebSession } from '../lib/api';
  import { confirm, failed, notify } from '../lib/feedback.svelte';
  import { guardNavigation, match, navigate, query as queryParam, route } from '../lib/router.svelte';
  import { refreshMetadata, session } from '../lib/session.svelte';
  import {
    categoriesOf,
    groupAnchor,
    groupRank,
    schema,
    settingGroups,
    settingsGroup,
    tabOf,
    type SettingsTab,
    type TabId,
  } from '../lib/settings-schema';
  import { EXPERT_GROUPS } from '../lib/schema/main';
  import SaveBar from '../components/settings/SaveBar.svelte';
  import { SETTING as PRESET, covered, presetById, presetId, presetNotice, withPreset } from '../lib/stream-presets';
  import { NATIVE_ENCODERS, NVIDIA_ENHANCEMENTS, type Setting } from '../lib/settings-types';

  const tabs = settingGroups;
  const byKey: ReadonlyMap<string, Setting> = new Map(schema.settings.map((setting) => [setting.key, setting]));
  const isDescribed = (key: string) => byKey.has(key);

  // Panels that are not settings keys, each a section of its tab like a group.
  const SECURITY = 'Sign-in and security';
  const INTEGRATIONS = 'Integrations';
  const CUSTOM_KEYS = 'Custom configuration keys';

  const section = $derived(match('/settings/:section')?.section ?? 'pc');
  /** A link to one setting opens the tab that holds it, whatever tab the link names. */
  const linked = $derived(byKey.get(new URLSearchParams(route.search).get('setting') ?? ''));
  const active = $derived<SettingsTab>(linked ? tabOf(linked.category) : tabOf(section));

  // Older links name a topic or a former tab (/settings/general, /settings/display):
  // show the tab under its own address, keeping the query and the anchor.
  $effect(() => {
    if (section !== active.id) navigate(`/settings/${active.id}${route.search}${location.hash}`, { replace: true });
  });

  /** What the host has stored; null until loaded. */
  let stored = $state<Config | null>(null);
  let loadError = $state('');
  let edits = $state<Edits>({});
  /** Invalid number or JSON drafts by key. Saving waits for correction. */
  let invalid = $state<Record<string, string>>({});
  // Search from the Settings home arrives as ?q=.
  let query = $state(queryParam('q') ?? '');
  // A later navigation that changes ?q= (the quick switcher's "Search all
  // settings", Back or Forward) replaces the search. Typing here never
  // touches the address, so it is kept until ?q= itself changes.
  let urlQuery = queryParam('q');
  $effect(() => {
    const next = queryParam('q');
    if (next === urlQuery) return;
    urlQuery = next;
    query = next ?? '';
  });
  let review = $state<'all' | 'changed' | 'invalid'>('all');
  // Keep reviewed rows mounted while they are corrected or reverted.
  let reviewKeys = $state<string[]>([]);
  // Only the settings changed from their defaults, saved or not.
  let customizedOnly = $state(false);
  let contentElement: HTMLDivElement;
  let saving = $state(false);
  let saved = $state(false);
  let restarting = $state(false);
  let latestLoad = 0;

  // Configuration keys are for people editing the settings file or following
  // a guide; the choice is remembered in this browser.
  const KEYS = 'butterpollo.settings.show-keys';
  let showKeys = $state(readShowKeys());
  function readShowKeys(): boolean {
    try {
      return localStorage.getItem(KEYS) === 'true';
    } catch {
      return false;
    }
  }
  function setShowKeys(show: boolean) {
    showKeys = show;
    try {
      if (show) localStorage.setItem(KEYS, 'true');
      else localStorage.removeItem(KEYS);
    } catch {
      // Storage may be unavailable (private windows); the switch still works for this visit.
    }
  }

  const config = $derived(stored ?? {});
  // The host fills the settings a preset covers, so under a preset they have
  // that preset's defaults: a row shows what is in effect, and Reset goes
  // back to the preset's value.
  const preset = $derived(presetById(presetId(has(edits, PRESET) ? edits[PRESET] : config[PRESET])));
  const described = $derived(withPreset(schema.settings, preset.id));
  const defaults: ReadonlyMap<string, Setting> = $derived(new Map(described.map((setting) => [setting.key, setting])));
  const values = $derived.by(() => {
    const current = currentValues(described, edits, config);
    // Automatic encoder settings follow this PC's GPUs.
    current[NATIVE_ENCODERS] = nativeEncoders(
      session.metadata?.gpus,
      String(current.adapter_name ?? ''),
      String(current.adapter_pnp_id ?? ''),
    );
    // RTX HDR and RTX Video settings show where the host offers them.
    current[NVIDIA_ENHANCEMENTS] = nvidiaEnhancements(session.metadata?.features);
    return current;
  });
  const changes = $derived(stored ? computeChanges(defaults, edits, stored, new Set(covered)) : {});
  const presetProblem = $derived(presetNotice(has(edits, PRESET) ? edits[PRESET] : config[PRESET]));
  /** Settings the preset covers that have a value of their own, even one equal to the preset's. */
  const handSet = $derived(
    preset.id === 'balanced'
      ? []
      : described.filter((setting) => covered.includes(setting.key) && isSet(setting, edits, config)),
  );
  const pending = $derived(new Set([...Object.keys(changes), ...Object.keys(invalid)]));
  const invalidCount = $derived(Object.keys(invalid).length);

  const changedIn = $derived.by(() => {
    const counts: Partial<Record<TabId, number>> = {};
    for (const key of pending) {
      const id = settingsGroup(byKey.get(key)?.category ?? 'advanced');
      counts[id] = (counts[id] ?? 0) + 1;
    }
    return counts;
  });

  /** Stored or newly added keys the schema does not describe. */
  const unknownKeys = $derived(
    [...new Set([...Object.keys(config), ...Object.keys(edits)])]
      .filter((key) => !byKey.has(key) && !NOT_SETTINGS.has(key))
      .sort(),
  );

  function visible(setting: Setting): boolean {
    if (!setting.visibleWhen) return true;
    try {
      return setting.visibleWhen(values);
    } catch {
      return true;
    }
  }

  /** Empty although the current choices make the host need a value. */
  function missing(setting: Setting): boolean {
    const value = values[setting.key];
    if (value !== null && value !== undefined && String(value).trim() !== '') return false;
    try {
      return setting.requiredWhen?.(values) ?? false;
    } catch {
      return false;
    }
  }

  interface Group {
    name: string;
    settings: Setting[];
  }

  // Every setting shows; only the expert groups start collapsed (EXPERT_GROUPS).
  let foldOpen = $state<Record<string, boolean>>({});

  /** Settings under their group headings, in the tab's order (groupRank), expert groups last. */
  function grouped(list: Setting[], tab: TabId): Group[] {
    const groups: Group[] = [];
    for (const setting of list) {
      const name = setting.group ?? '';
      let group = groups.find((candidate) => candidate.name === name);
      if (!group) {
        group = { name, settings: [] };
        groups.push(group);
      }
      group.settings.push(setting);
    }
    return groups.sort((a, b) => groupRank(tab, a.name) - groupRank(tab, b.name));
  }

  const inTab = $derived(described.filter((setting) => settingsGroup(setting.category) === active.id));
  // Keep a pending control reachable if editing its parent makes it inactive.
  const shown = $derived(inTab.filter((setting) => visible(setting) || pending.has(setting.key)));
  const shownGroups = $derived(grouped(shown, active.id));

  /** The tab's sections in page order, for its sticky contents: groups, and the panels that are not settings. */
  const sections = $derived([
    ...(active.id === 'advanced' ? [SECURITY] : []),
    ...shownGroups.map((group) => group.name || active.label),
    ...(active.id === 'picture' ? [INTEGRATIONS] : []),
    ...(active.id === 'advanced' ? [CUSTOM_KEYS] : []),
  ]);

  // ---------- the sticky contents: where the page is, and jumps to a section ----------

  let frame = $state<HTMLDivElement>();
  let track = $state<HTMLElement>();
  /** The height of the console's own sticky header above the page (on narrow windows), and of the contents bar. */
  let headerHeight = $state(0);
  let contentsHeight = $state(0);
  let current = $state('');

  function measure() {
    // The console's navigation sticks to the top of narrow windows; the contents stick below it.
    const aside = document.querySelector('aside');
    headerHeight = aside && getComputedStyle(aside).position === 'sticky' ? Math.round(aside.getBoundingClientRect().height) : 0;
  }

  /** The section at the top of the window, under the sticky bars. */
  function spy() {
    if (!frame) return;
    const line = headerHeight + contentsHeight + 24;
    let found = sections[0] ?? '';
    for (const name of sections) {
      const element = document.getElementById(groupAnchor(name));
      if (element && element.getBoundingClientRect().top <= line) found = name;
    }
    current = found;
  }

  // Keep the current section's link in view in a contents bar that scrolls sideways.
  $effect(() => {
    const link = current && track?.querySelector<HTMLElement>(`[data-section="${CSS.escape(current)}"]`);
    if (!link || !track) return;
    const { left, right } = link.getBoundingClientRect();
    const bar = track.getBoundingClientRect();
    if (left < bar.left || right > bar.right - 32) track.scrollLeft += left - bar.left - 16;
  });

  onMount(() => {
    measure();
    // The header grows while its menu is open on a narrow window.
    const aside = document.querySelector('aside');
    const observer = typeof ResizeObserver === 'function' && aside ? new ResizeObserver(() => measure()) : null;
    if (aside) observer?.observe(aside);
    let frameRequest = 0;
    const onScroll = () => {
      cancelAnimationFrame(frameRequest);
      frameRequest = requestAnimationFrame(spy);
    };
    const onResize = () => {
      measure();
      onScroll();
    };
    window.addEventListener('scroll', onScroll, { passive: true });
    window.addEventListener('resize', onResize);
    return () => {
      cancelAnimationFrame(frameRequest);
      observer?.disconnect();
      window.removeEventListener('scroll', onScroll);
      window.removeEventListener('resize', onResize);
    };
  });

  const reducedMotion = () => typeof matchMedia === 'function' && matchMedia('(prefers-reduced-motion: reduce)').matches;

  /** Show a section: open it if it folds, then bring it under the sticky bars. */
  async function reveal(name: string, smooth = true) {
    if (EXPERT_GROUPS.has(name)) foldOpen[name] = true;
    if (name === INTEGRATIONS) integrationsOpen = true;
    if (name === CUSTOM_KEYS) customOpen = true;
    await tick();
    const element = document.getElementById(groupAnchor(name));
    if (!element) return;
    element.scrollIntoView({ block: 'start', behavior: smooth && !reducedMotion() ? 'smooth' : 'auto' });
    current = name;
    if (!smooth) hold(element);
  }

  /** On arrival, keep a section in place while the panels above it finish loading, until the user scrolls. */
  function hold(element: HTMLElement) {
    if (typeof ResizeObserver !== 'function' || !frame) return;
    const observer = new ResizeObserver(() => element.scrollIntoView({ block: 'start' }));
    const events = ['wheel', 'touchstart', 'keydown', 'pointerdown'] as const;
    const stop = () => {
      observer.disconnect();
      clearTimeout(timer);
      for (const name of events) window.removeEventListener(name, stop, true);
    };
    const timer = setTimeout(stop, 2000);
    for (const name of events) window.addEventListener(name, stop, { capture: true, passive: true });
    observer.observe(frame);
  }

  function jump(event: MouseEvent, name: string) {
    if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
    event.preventDefault();
    history.replaceState(history.state, '', `#${groupAnchor(name)}`);
    void reveal(name);
  }

  // A link to one setting (?setting=key, from the Settings home's search)
  // opens its group, then brings the row into view and marks it. A link to a
  // section (#updates) opens and shows that section.
  // A setting the current choices hide is shown through search instead.
  let jumped = '';
  $effect(() => {
    const target = route.path + route.search;
    if (stored === null || section !== active.id || jumped === target) return;
    jumped = target;
    const key = new URLSearchParams(route.search).get('setting');
    if (!key) {
      const anchor = location.hash.slice(1);
      const name = anchor && sections.find((candidate) => groupAnchor(candidate) === anchor);
      if (name) void reveal(name, false);
      return;
    }
    const group = byKey.get(key)?.group;
    if (group) foldOpen[group] = true;
    void tick().then(() => {
      const row = document.querySelector<HTMLElement>(`[data-setting="${CSS.escape(key)}"]`);
      if (!row) {
        query = key;
        return;
      }
      row.scrollIntoView({ block: 'center' });
      row.querySelector<HTMLElement>('input, select, textarea, button')?.focus({ preventScroll: true });
      const mark = getComputedStyle(row).getPropertyValue('--accent-soft').trim();
      if (mark && !reducedMotion()) row.animate([{ backgroundColor: mark }, { backgroundColor: 'transparent' }], { duration: 1800, easing: 'ease-out' });
    });
  });

  // ---------- panels that are not settings ----------

  let integrationsOpen = $state(false);
  let customOpen = $state<boolean | null>(null);
  // Custom keys open by themselves while there are any.
  const customShown = $derived(customOpen ?? unknownKeys.length > 0);

  let sessions = $state<WebSession[] | null>(null);
  let sessionsError = $state('');
  let sessionsAsked = false;
  const username = $derived(sessions?.find((item) => item.current)?.username ?? '');
  async function loadSessions() {
    try {
      sessions = (await api.auth.sessions()).sessions;
      sessionsError = '';
    } catch (error) {
      sessionsError = reason(error);
    }
  }
  $effect(() => {
    if (active.id !== 'advanced' || sessionsAsked) return;
    sessionsAsked = true;
    void loadSessions();
  });

  // ---------- search ----------

  const words = $derived(searchWords(query));
  const searching = $derived(words.length > 0);
  const browsingResults = $derived(searching || review !== 'all' || customizedOnly);
  const listingCustomized = $derived(customizedOnly && review === 'all' && !searching);
  // A setting put back to its default while the filter is on stays listed until the filter is
  // turned on again, so its row does not vanish under the pointer.
  const inReview = (key: string) => {
    if (review !== 'all') return reviewKeys.includes(key);
    if (!customizedOnly) return true;
    if (reviewKeys.includes(key) || pending.has(key) || unknownKeys.includes(key)) return true;
    const setting = defaults.get(key);
    return setting !== undefined && differsFromDefault(setting, edits, config);
  };
  function setCustomizedOnly(checked: boolean) {
    customizedOnly = checked;
    review = 'all';
    reviewKeys = checked
      ? [...described.filter((setting) => differsFromDefault(setting, edits, config) || pending.has(setting.key)).map((setting) => setting.key), ...unknownKeys]
      : [];
  }

  const results = $derived.by(() => {
    if (!browsingResults) return [];
    return tabs
      .map((tab) => ({
        tab,
        settings: described.filter(
          (setting) =>
            settingsGroup(setting.category) === tab.id &&
            inReview(setting.key) && categoriesOf(setting).some((shownUnder) => matchesSetting(setting, shownUnder, words)),
        ),
        unknown: tab.id === 'advanced' ? unknownKeys.filter((key) => inReview(key) && matchesWords(key, words)) : [],
      }))
      .filter((result) => result.settings.length > 0 || result.unknown.length > 0);
  });
  const resultCount = $derived(new Set(results.flatMap((result) => [
    ...result.settings.map((setting) => setting.key), ...result.unknown,
  ])).size);

  async function reviewChanges(mode: 'changed' | 'invalid') {
    query = '';
    customizedOnly = false;
    reviewKeys = mode === 'invalid' ? Object.keys(invalid) : [...pending];
    review = mode;
    await tick();
    contentElement?.focus();
    contentElement?.scrollIntoView({ block: 'start' });
  }

  // ---------- loading and editing ----------

  async function load() {
    // A load that started before a save must not put the old values back.
    const id = ++latestLoad;
    try {
      const config = settingsOnly(await api.config.get());
      if (id !== latestLoad) return;
      stored = config;
      loadError = '';
    } catch (error) {
      if (id === latestLoad) loadError = error instanceof Error ? error.message : String(error);
    }
  }

  onMount(() => {
    void load();
    // Fresh display, GPU and audio lists for the pickers.
    refreshMetadata().catch(() => {});
  });

  function edit(key: string, value: ConfigValue) {
    edits[key] = value;
  }
  function reset(key: string) {
    edits[key] = null;
    delete invalid[key];
  }
  function undo(key: string) {
    delete edits[key];
    delete invalid[key];
  }
  function setInvalid(key: string, text: string | null) {
    if (text === null) delete invalid[key];
    else invalid[key] = text;
  }
  function removeUnknown(key: string) {
    if (has(config, key)) edits[key] = null;
    else delete edits[key];
  }
  async function discard() {
    const count = pending.size;
    const ok = await confirm({
      title: 'Discard unsaved changes?',
      message: `${count} unsaved ${count === 1 ? 'change is' : 'changes are'} lost. Rubylight keeps its current settings.`,
      confirm: 'Discard',
      danger: true,
    });
    if (!ok) return;
    edits = {};
    invalid = {};
    review = 'all';
  }

  function open(id: string, event?: MouseEvent) {
    if (event) {
      if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
      event.preventDefault();
    }
    query = '';
    review = 'all';
    customizedOnly = false;
    navigate(`/settings/${id}`);
  }

  async function save() {
    const base = stored;
    if (!base || saving || pending.size === 0 || invalidCount > 0) return;
    const body = $state.snapshot(changes) as Config;
    // Edits made while the request is out stay unsaved.
    const submitted = $state.snapshot(edits) as Edits;
    saving = true;
    latestLoad++;
    try {
      const result = await api.config.patch(body);
      stored = applyChanges(stored ?? base, body);
      for (const [key, value] of Object.entries(submitted)) {
        if (has(edits, key) && sameValue(edits[key], value)) delete edits[key];
      }
      saved = result.restart_required !== false;
      if (!saved) notify('Settings saved.', 'ok');
      if (result.warning) notify(result.warning, 'warn', 8000);
      void load();
    } catch (error) {
      failed('Saving settings failed', error);
    } finally {
      saving = false;
    }
  }

  async function restart() {
    const ok = await confirm({
      title: 'Restart Rubylight?',
      message: 'Streams in progress disconnect. This console reconnects by itself when Rubylight is back.',
      confirm: 'Restart',
    });
    if (!ok) return;
    restarting = true;
    try {
      await api.host.restart();
      saved = false;
      notify('Rubylight is restarting. This console reconnects when it is back.', 'info', 8000);
    } catch (error) {
      failed('Restart failed', error);
    } finally {
      restarting = false;
    }
  }

  // Leaving the page with unsaved changes asks first; switching tabs
  // keeps the edits.
  $effect(() => {
    if (pending.size === 0) return;
    const warn = (event: BeforeUnloadEvent) => event.preventDefault();
    window.addEventListener('beforeunload', warn);
    const unguard = guardNavigation((to) =>
      // Other tabs keep the edits; the Settings home is another page.
      to.startsWith('/settings/')
        ? true
        : confirm({
            title: 'Leave without saving?',
            message: `${pending.size} unsaved ${pending.size === 1 ? 'change is' : 'changes are'} lost.`,
            confirm: 'Leave',
            danger: true,
          }),
    );
    return () => {
      window.removeEventListener('beforeunload', warn);
      unguard();
    };
  });
</script>

{#snippet row(setting: Setting, inactive: boolean)}
  <SettingRow
    {setting}
    value={normalise(setting, effective(setting, edits, config))}
    stored={has(config, setting.key)}
    changed={has(changes, setting.key)}
    resetting={edits[setting.key] === null && has(config, setting.key)}
    differs={differsFromDefault(setting, edits, config)}
    showKey={showKeys || (searching && matchesWords(setting.key, words))}
    {inactive}
    unavailable={inactive && setting.hostGated === true}
    missing={missing(setting)}
    caution={caution(setting, values)}
    invalidText={invalid[setting.key]}
    onchange={(value) => edit(setting.key, value)}
    onreset={() => reset(setting.key)}
    onundo={() => undo(setting.key)}
    oninvalid={(text) => setInvalid(setting.key, text)}
  />
  {#if setting.key === PRESET}
    {#if presetProblem}
      <p class="preset-note preset-error" role="alert" data-preset-error><strong>Not a preset.</strong> {presetProblem}</p>
    {/if}
    <p class="preset-note" data-preset-note>
      {preset.description}
      {#if handSet.length > 0}
        <strong>Custom:</strong> set by hand, so they win over the preset: {handSet.map((item) => item.label).join(', ')}.
      {/if}
    </p>
  {/if}
{/snippet}

{#snippet rows(group: Group)}
  <div class="rows">
    <!-- Keyed by object: a key may be listed in two categories. -->
    {#each group.settings as setting (setting)}
      {@render row(setting, !visible(setting))}
    {/each}
  </div>
{/snippet}

{#snippet otherKeys(keys: string[], adding: boolean)}
  <UnknownKeys
    {keys}
    stored={config}
    {edits}
    {changes}
    {isDescribed}
    {adding}
    onedit={edit}
    onremove={removeUnknown}
    onundo={undo}
  />
{/snippet}

{#snippet viewLogs()}<Button size="sm" href="/logs" icon="logs">View logs</Button>{/snippet}

{#snippet group(item: Group)}
  {@const name = item.name || active.label}
  {@const anchor = groupAnchor(name)}
  {#if active.id === 'pc' && item.name === 'Updates'}
    <!-- The update settings above the release status they act on, as on the Settings home. -->
    <UpdatesPanel id={anchor} title={name}>
      {#snippet settings()}{@render rows(item)}{/snippet}
    </UpdatesPanel>
  {:else if active.id === 'pc' && item.name === 'Sleep and wake'}
    <PowerPanel id={anchor} title={name}>
      {#snippet settings()}{@render rows(item)}{/snippet}
    </PowerPanel>
  {:else if EXPERT_GROUPS.has(item.name)}
    {@const fold = foldStatus(item.settings, edits, config, pending)}
    <!-- Opened by the user. A customized value opens it until the user closes it; an unsaved one keeps it open. -->
    <Disclosure
      id={anchor}
      title={name}
      level={3}
      flush
      meta={`${item.settings.length} ${item.settings.length === 1 ? 'setting' : 'settings'}${fold.customized ? `, ${fold.customized} customized` : ''}${fold.unsaved ? `, ${fold.unsaved} unsaved` : ''}`}
      open={(foldOpen[item.name] ?? fold.customized > 0) || fold.mustOpen}
      ontoggle={(open) => (foldOpen[item.name] = open)}
    >
      {@render rows(item)}
    </Disclosure>
  {:else}
    <Panel id={anchor} title={name} level={3} flush actions={item.name === 'Logging' ? viewLogs : undefined}>
      {@render rows(item)}
    </Panel>
  {/if}
{/snippet}

<PageHeader title="All settings" subtitle="Every setting Rubylight reads, by topic.">
  {#snippet actions()}
    <label class="show-keys">
      <span>Customized only</span>
      <Switch checked={customizedOnly} onchange={setCustomizedOnly} />
    </label>
    <label class="show-keys">
      <span>Show config keys</span>
      <Switch checked={showKeys} onchange={setShowKeys} />
    </label>
  {/snippet}
</PageHeader>

<div
  class="frame"
  bind:this={frame}
  style:--sticky-top="{headerHeight}px"
  style:--sticky-offset="{headerHeight + contentsHeight + 16}px"
>
  <div class="layout">
    <div class="side">
      <div class="search">
        <Icon name="search" size={16} />
        <input
          class="input"
          type="search"
          placeholder="Search settings"
          aria-label="Search settings"
          autocomplete="off"
          spellcheck="false"
          bind:value={query}
          oninput={() => (review = 'all')}
        />
      </div>

      <label class="picker">
        <span>Settings tab</span>
        <select aria-label="Settings tab" class="select" onchange={(event) => open(event.currentTarget.value)}>
          {#if browsingResults}<option value="" disabled selected>All tabs</option>{/if}
          {#each tabs as tab (tab.id)}
            {@const count = changedIn[tab.id] ?? 0}
            <option value={tab.id} selected={!browsingResults && tab.id === active.id}>
              {tab.label}{count ? ` (${count} unsaved)` : ''}
            </option>
          {/each}
        </select>
      </label>

      <nav class="tabs" aria-label="Settings tabs">
        <ul>
          {#each tabs as tab (tab.id)}
            {@const count = changedIn[tab.id] ?? 0}
            <li>
              <a
                href="/settings/{tab.id}"
                aria-current={!browsingResults && tab.id === active.id ? 'page' : undefined}
                onclick={(event) => open(tab.id, event)}
              >
                <span class="name">
                  {tab.label}
                  {#if count}
                    <span class="dot" aria-hidden="true"></span>
                    <span class="visually-hidden">, {count} unsaved</span>
                  {/if}
                </span>
              </a>
            </li>
          {/each}
        </ul>
      </nav>
    </div>

    <div class="content" bind:this={contentElement} tabindex="-1">
      {#if loadError}
        <div class="notice danger load-error" role="alert">
          <span>{stored ? 'Could not refresh the settings' : 'Could not load the settings'}: {loadError}</span>
          <Button size="sm" onclick={load}>Retry</Button>
        </div>
      {/if}

      {#if stored === null}
        {#if !loadError}<p class="placeholder muted">Loading settings…</p>{/if}
      {:else if browsingResults}
        <div class="results-heading">
          <div>
            <h2>{review === 'invalid' ? 'Review errors' : review === 'changed' ? 'Review changes' : listingCustomized ? 'Customized settings' : 'Search results'}</h2>
            <p class="muted" role="status">
              {#if review !== 'all' && pending.size === 0}
                No unsaved changes.
              {:else if review === 'invalid' && invalidCount === 0}
                All errors corrected. Review your values, then save your changes.
              {:else}
                {resultCount} {resultCount === 1 ? 'setting' : 'settings'}{searching ? ` matching “${query.trim()}”` : ' across all tabs'}
              {/if}
            </p>
          </div>
          <Button size="sm" variant="ghost" onclick={() => open(active.id)}>All settings</Button>
        </div>
        {#if results.length === 0}
          <Panel>
            <EmptyState icon="search" title={review === 'invalid' ? 'All corrections complete' : review === 'changed' ? 'No unsaved changes' : listingCustomized ? 'All settings are at their defaults' : 'No settings match'}>
              <p>{review === 'invalid' ? 'You can save your changes now.' : review === 'changed' ? 'Your settings are up to date.' : listingCustomized ? 'Settings you change from their defaults appear here.' : 'Search looks at names, config keys, descriptions, topics and menu choices.'}</p>
            </EmptyState>
          </Panel>
        {:else}
          {#each results as result (result.tab.id)}
            <Panel title={result.tab.label} flush>
              {#snippet actions()}
                <Button size="sm" variant="ghost" onclick={() => open(result.tab.id)}>
                  Open<span class="visually-hidden">{` ${result.tab.label}`}</span>
                </Button>
              {/snippet}
              <div class="rows">
                {#each result.settings as setting (setting)}
                  {@render row(setting, !visible(setting))}
                {/each}
              </div>
              {#if result.unknown.length}
                {@render otherKeys(result.unknown, false)}
              {/if}
            </Panel>
          {/each}
        {/if}
      {:else}
        <h2 class="visually-hidden">{active.label}</h2>
        {#if sections.length > 1}
          <nav class="contents" aria-label="{active.label} sections" bind:clientHeight={contentsHeight}>
            <div class="track" bind:this={track}>
              {#each sections as name (name)}
                <a
                  href="#{groupAnchor(name)}"
                  data-section={name}
                  aria-current={current === name ? 'location' : undefined}
                  onclick={(event) => jump(event, name)}
                >{name}</a>
              {/each}
            </div>
          </nav>
        {/if}

        {#if active.id === 'advanced'}
          <section class="security" id={groupAnchor(SECURITY)} aria-label={SECURITY}>
            <div class="pair">
              <SessionsPanel {sessions} error={sessionsError} onchange={loadSessions} />
              <PasswordPanel {username} />
            </div>
            <div class="row links">
              <Button href="/tokens" icon="key">API tokens</Button>
              <Button href="/webhooks" icon="link">Webhooks</Button>
            </div>
          </section>
        {/if}

        {#if inTab.length > 0 && shown.length === 0}
          <Panel><p class="muted">None of these settings apply with the current choices.</p></Panel>
        {/if}
        {#each shownGroups as item (item.name)}
          {@render group(item)}
        {/each}

        {#if active.id === 'picture'}
          <!-- Installs and status, not settings keys. -->
          <Disclosure
            id={groupAnchor(INTEGRATIONS)}
            title={INTEGRATIONS}
            meta="Vulkan HDR layer and RTSS"
            level={3}
            open={integrationsOpen}
            ontoggle={(open) => (integrationsOpen = open)}
          >
            <div class="integration-panels">
              <VulkanPanel />
              <LimiterPanel />
            </div>
          </Disclosure>
        {/if}
        {#if active.id === 'network'}
          <!-- Status, not settings keys: what the router mapped and how devices reach this PC. -->
          <InternetPlayPanel />
        {/if}
        {#if active.id === 'advanced'}
          <Disclosure
            id={groupAnchor(CUSTOM_KEYS)}
            title={CUSTOM_KEYS}
            meta="{unknownKeys.length} stored"
            level={3}
            flush
            open={customShown}
            ontoggle={(open) => (customOpen = open)}
          >
            {@render otherKeys(unknownKeys, true)}
          </Disclosure>
        {/if}
      {/if}
    </div>
  </div>
</div>

{#if stored}
  <SaveBar
    pending={pending.size}
    invalid={invalidCount}
    {saving}
    {saved}
    {restarting}
    onreview={() => reviewChanges(invalidCount ? 'invalid' : 'changed')}
    ondiscard={discard}
    onsave={save}
    onrestart={restart}
    ondismiss={() => (saved = false)}
  />
{/if}

<style>
  .preset-note {
    margin: 0;
    padding: 0 var(--space-5) var(--space-3);
    color: var(--muted);
    font-size: var(--text-sm);
  }
  .preset-note strong { color: var(--ink); }
  .preset-error, .preset-error strong { color: var(--danger); }
  .frame {
    /* The layout follows the room the page has, whatever the shell around it. */
    container-type: inline-size;
    min-width: 0;
  }
  .layout {
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    gap: var(--space-5);
    align-items: start;
  }
  .side {
    display: grid;
    gap: var(--space-3);
    min-width: 0;
  }
  /* As wide as the search on the Settings home, so the two pages line up. */
  .search {
    max-width: 640px;
    position: relative;
    color: var(--muted);
  }
  .search :global(svg) {
    position: absolute;
    left: 10px;
    top: 50%;
    transform: translateY(-50%);
    pointer-events: none;
  }
  .search .input {
    padding-left: var(--space-6);
  }
  .picker {
    display: none;
    gap: 6px;
    font-size: var(--text-sm);
    font-weight: var(--weight-medium);
  }
  .tabs ul {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-1);
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .tabs a {
    display: grid;
    gap: 1px;
    padding: var(--space-3) 14px;
    min-height: var(--control-height);
    border-radius: var(--radius);
    color: var(--ink-2);
    text-decoration: none;
  }
  .tabs a:hover {
    background: var(--panel);
    color: var(--ink);
  }
  .tabs a[aria-current='page'] {
    background: var(--panel);
    color: var(--ink);
    box-shadow: inset 0 -3px 0 var(--accent);
  }
  .name {
    display: flex;
    align-items: center;
    gap: 6px;
    font-weight: var(--weight-medium);
    font-size: var(--text-sm);
  }
  .dot {
    flex: none;
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--accent);
  }
  .content {
    display: grid;
    gap: var(--space-4);
    align-content: start;
    min-width: 0;
    scroll-margin-top: var(--space-4);
  }
  .content:focus {
    outline: none;
  }
  /* The tab's sections, under the console's own sticky header on narrow windows.
     Wrapped on wide pages; one row that scrolls sideways on narrow ones. */
  .contents {
    position: sticky;
    top: var(--sticky-top, 0px);
    z-index: var(--z-sticky);
    /* The row scrolls inside; it never widens the page. */
    min-width: 0;
    margin-inline: calc(-1 * var(--space-2));
    padding: var(--space-2);
    background: var(--bg);
    border-bottom: 1px solid var(--line);
  }
  .track {
    display: flex;
    flex-wrap: wrap;
    gap: 2px var(--space-1);
  }
  .track::-webkit-scrollbar {
    display: none;
  }
  .contents a {
    display: inline-flex;
    align-items: center;
    flex: none;
    min-height: var(--control-height);
    padding: 0 14px;
    border: 1px solid transparent;
    border-radius: var(--radius-pill);
    color: var(--ink-2);
    font-size: var(--text-sm);
    font-weight: var(--weight-medium);
    text-decoration: none;
    white-space: nowrap;
    transition: background var(--duration-fast), color var(--duration-fast);
  }
  .contents a:hover {
    background: var(--panel);
    color: var(--ink);
  }
  .contents a[aria-current='location'] {
    border-color: var(--line-strong);
    background: var(--panel);
    color: var(--ink);
  }
  .security {
    display: grid;
    gap: var(--space-3);
    min-width: 0;
    scroll-margin-top: var(--sticky-offset, var(--space-4));
    container-type: inline-size;
  }
  /* Two panels side by side on wide screens, each as tall as its content. */
  .pair {
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    gap: var(--space-4);
    align-items: start;
  }
  @container (min-width: 900px) {
    .pair {
      grid-template-columns: repeat(2, minmax(0, 1fr));
    }
  }
  .results-heading {
    display: flex;
    justify-content: space-between;
    align-items: center;
    flex-wrap: wrap;
    gap: var(--space-2);
  }
  .results-heading h2 {
    font-size: var(--text-lg);
  }
  .results-heading p {
    font-size: var(--text-sm);
  }
  .load-error {
    align-items: center;
    justify-content: space-between;
    flex-wrap: wrap;
  }
  .placeholder {
    padding: var(--space-5);
    font-size: var(--text-sm);
  }
  .rows {
    display: grid;
  }
  .rows > :global(.setting + .setting) {
    border-top: 1px solid var(--line);
  }
  .integration-panels {
    display: grid;
    gap: var(--space-4);
    min-width: 0;
  }
  .show-keys {
    display: inline-flex;
    align-items: center;
    gap: var(--space-3);
    min-height: 36px;
    font-size: var(--text-sm);
    font-weight: var(--weight-medium);
    color: var(--ink-2);
    cursor: pointer;
  }
  @container (max-width: 720px) {
    .layout {
      grid-template-columns: minmax(0, 1fr);
      gap: var(--space-4);
    }
    .side {
      position: static;
      grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
      align-items: end;
    }
    .picker {
      display: grid;
    }
    .tabs {
      display: none;
    }
    /* The names that do not fit fade out at the edge, a sign that the row scrolls. */
    .track {
      flex-wrap: nowrap;
      overflow-x: auto;
      scrollbar-width: none;
      padding-right: var(--space-6);
      mask-image: linear-gradient(to left, transparent, #000 var(--space-6));
    }
  }
  @media (max-width: 520px) {
    .side {
      grid-template-columns: minmax(0, 1fr);
    }
    .show-keys {
      min-height: var(--control-height);
    }
    .contents {
      margin-inline: calc(-1 * var(--space-4));
      padding-inline: var(--space-4);
    }
  }
</style>
