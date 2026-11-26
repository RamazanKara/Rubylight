<script lang="ts">
  import { onMount, tick } from 'svelte';
  import Button from '../components/Button.svelte';
  import Disclosure from '../components/Disclosure.svelte';
  import PageHeader from '../components/PageHeader.svelte';
  import Panel from '../components/Panel.svelte';
  import GameSourcesPanel from '../components/settings/GameSourcesPanel.svelte';
  import PresetNote from '../components/settings/PresetNote.svelte';
  import SaveBar from '../components/settings/SaveBar.svelte';
  import SettingRow from '../components/settings/SettingRow.svelte';
  import DisclosureMeta from '../components/settings/DisclosureMeta.svelte';
  import SettingsContentsTrack from '../components/settings/SettingsContentsTrack.svelte';
  import SettingsResults from '../components/settings/SettingsResults.svelte';
  import SettingsSidebar from '../components/settings/SettingsSidebar.svelte';
  import Switch from '../components/settings/Switch.svelte';
  import UnknownKeys from '../components/settings/UnknownKeys.svelte';
  import LimiterPanel from '../components/maintenance/LimiterPanel.svelte';
  import PasswordPanel from '../components/maintenance/PasswordPanel.svelte';
  import PowerPanel from '../components/maintenance/PowerPanel.svelte';
  import SessionsPanel from '../components/maintenance/SessionsPanel.svelte';
  import UpdatesPanel from '../components/maintenance/UpdatesPanel.svelte';
  import VulkanPanel from '../components/maintenance/VulkanPanel.svelte';
  import NvidiaPanel from '../components/maintenance/NvidiaPanel.svelte';
  import InternetPlayPanel from '../components/maintenance/InternetPlayPanel.svelte';
  import { reason } from '../components/maintenance/util';
  import { matchesWords, searchWords } from '../components/settings/search';
  import * as page from '../components/settings/settings-page';
  import {
    applyChanges, askedFor, caution, computeChanges, currentValues, differsFromDefault,
    foldStatus, has, isSet, sameValue, settingsOnly, withHostGates, type Edits,
  } from '../components/settings/values';
  import { api, type Config, type ConfigValue, type WebSession } from '../lib/api';
  import { failed, notify } from '../lib/feedback.svelte';
  import { match, navigate, query as queryParam, route } from '../lib/router.svelte';
  import { refreshMetadata, session } from '../lib/session.svelte';
  import {
    GAME_SOURCES, groupAnchor, movedAnchor, placeOf, schema, settingGroups,
    tabLayout, tabOf, topicGroup, type LayoutGroup, type SettingsTab,
  } from '../lib/settings-schema';
  import { DEBUG_GROUP, FOLDED_GROUPS, TRAILING_GROUPS } from '../lib/schema/main';
  import { GAMES_PAGE_KEY_SET } from '../lib/schema/tiers';
  import { SETTING as PRESET, covered, presetById, presetId, presetNotice, withPreset } from '../lib/stream-presets';
  import type { Setting } from '../lib/settings-types';

  const tabs = settingGroups;
  const byKey: ReadonlyMap<string, Setting> = new Map(schema.settings.map((setting) => [setting.key, setting]));
  const isDescribed = (key: string) => byKey.has(key);

  // Panels that are not settings keys, each a section of its tab like a group.
  const SECURITY = 'Sign-in and security';
  const INTEGRATIONS = 'Integrations';
  const CUSTOM_KEYS = 'Custom configuration keys';

  const section = $derived(match('/settings/:section')?.section ?? 'pc');
  /** A link to one setting opens the tab where it shows, whatever tab the link names. */
  const linked = $derived(byKey.get(new URLSearchParams(route.search).get('setting') ?? ''));
  const active = $derived<SettingsTab>(linked ? tabOf(placeOf(linked).tab) : tabOf(section));

  // Older links name a topic or a former tab (/settings/general, /settings/display):
  // show the tab under its own address, keeping the query and the anchor. A link to
  // a group that moved (/settings/advanced#picture-amd-amf) follows it there.
  $effect(() => {
    const moved = linked ? null : movedAnchor(active.id, location.hash.slice(1));
    if (moved) navigate(`/settings/${moved.tab}${route.search}#${moved.anchor}`, { replace: true });
    else if (section !== active.id) navigate(`/settings/${active.id}${route.search}${location.hash}`, { replace: true });
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

  let showKeys = $state(page.readShowKeys());
  function setShowKeys(show: boolean) {
    showKeys = show;
    page.writeShowKeys(show);
  }

  const config = $derived(stored ?? {});
  // The host fills the settings a preset covers, so under a preset they have
  // that preset's defaults: a row shows what is in effect, and Reset goes
  // back to the preset's value.
  const preset = $derived(presetById(presetId(has(edits, PRESET) ? edits[PRESET] : config[PRESET])));
  const described = $derived(withPreset(schema.settings, preset.id));
  const defaults: ReadonlyMap<string, Setting> = $derived(new Map(described.map((setting) => [setting.key, setting])));
  // Encoder, NVIDIA and virtual display settings follow what this PC can run.
  const values = $derived(withHostGates(currentValues(described, edits, config), session.metadata));
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
  const changedIn = $derived(page.countChangedByTab(pending, byKey));
  const unknownKeys = $derived(page.findUnknownKeys(config, edits, byKey));

  const visible = (setting: Setting) => page.isSettingVisible(setting, values);
  const missing = (setting: Setting) => page.isSettingMissing(setting, values);

  type Group = LayoutGroup;

  // Opened by the user. A tab's groups in TRAILING_GROUPS and FOLDED_GROUPS start collapsed; a changed
  // group starts open.
  let foldOpen = $state<Record<string, boolean>>({});
  /** The launchers' option groups a link opened in full (the Games tab otherwise lists them as one row). */
  let launcherOpen = $state<Record<string, boolean>>({});

  /** A setting edited inside another one's row (Setting.partOf) is listed as that row. */
  const listed = (setting: Setting): Setting => (setting.partOf ? (defaults.get(setting.partOf) ?? setting) : setting);
  const asked = (setting: Setting) => askedFor(setting, edits, config, pending);
  // Keep a pending control reachable if editing its parent makes it inactive.
  const reachable = (setting: Setting) => visible(setting) || pending.has(setting.key);
  const layout = $derived(
    tabLayout(
      active.id,
      described.filter((setting) => !setting.partOf),
      { changed: asked, visible: reachable, revealed: new Set(Object.keys(launcherOpen).filter((name) => launcherOpen[name])) },
    ),
  );
  const debugGroup = $derived(layout.groups.find((group) => group.name === DEBUG_GROUP));
  const tabGroups = $derived(layout.groups.filter((group) => group.name !== DEBUG_GROUP));
  const tabEmpty = $derived(layout.groups.length === 0);

  /** The tab's sections in page order, for its sticky contents: groups, and the panels that are not settings. */
  const sections = $derived(
    page.buildTabSections(active.id, active.label, tabGroups, !!debugGroup, {
      security: SECURITY,
      debug: DEBUG_GROUP,
      integrations: INTEGRATIONS,
      customKeys: CUSTOM_KEYS,
    }),
  );

  // ---------- the sticky contents: where the page is, and jumps to a section ----------

  let frame = $state<HTMLDivElement>();
  let headerHeight = $state(0);
  let contentsHeight = $state(0);
  let current = $state('');

  const measure = () => { headerHeight = page.measureStickyHeader(); };
  const spy = () => {
    const found = page.activeSection(frame, sections, headerHeight, contentsHeight);
    if (found !== null) current = found;
  };
  onMount(() => page.observeStickyHeader(measure, spy));

  /** Show a section: open it if it folds, then bring it under the sticky bars. */
  async function reveal(name: string, smooth = true) {
    if (TRAILING_GROUPS.has(name) || FOLDED_GROUPS.has(name)) foldOpen[name] = true;
    if (name === INTEGRATIONS) integrationsOpen = true;
    if (name === CUSTOM_KEYS) customOpen = true;
    await tick();
    if (page.scrollSectionIntoView(frame, name, smooth)) current = name;
  }

  function jump(event: MouseEvent, name: string) {
    if (!page.isPlainClick(event)) return;
    event.preventDefault();
    history.replaceState(history.state, '', `#${groupAnchor(name)}`);
    void reveal(name);
  }

  // A link to one setting (?setting=key, from the Settings home's search) opens its group and marks it.
  // A link to a section (#updates) opens and shows that section; hidden settings fall back to search.
  let jumped = '';
  $effect(() => {
    const target = route.path + route.search;
    if (stored === null || section !== active.id || jumped === target) return;
    jumped = target;
    const key = new URLSearchParams(route.search).get('setting');
    if (!key) {
      const anchor = location.hash.slice(1);
      const launcherGroup = page.findAnchorLauncherGroup(anchor, active.id, described);
      if (launcherGroup) launcherOpen[launcherGroup] = true;
      const name = anchor && sections.find((candidate) => groupAnchor(candidate) === anchor);
      if (name) void reveal(name, false);
      return;
    }
    const picked = byKey.get(key);
    if (picked) {
      if (GAMES_PAGE_KEY_SET.has(key)) launcherOpen[topicGroup(picked)] = true;
      foldOpen[placeOf(picked).group] = true;
    }
    void tick().then(() => { if (!page.focusSettingRow(key)) query = key; });
  });

  // ---------- panels that are not settings ----------

  let integrationsOpen = $state(false);
  const nvidiaShown = $derived(page.isNvidiaPanelShown(session.metadata));
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
    reviewKeys = checked ? page.customizedReviewKeys(described, edits, config, pending, unknownKeys) : [];
  }

  const results = $derived.by(() =>
    browsingResults ? page.buildSearchResults(tabs, described, unknownKeys, words, inReview, listed) : [],
  );
  const resultCount = $derived(page.countSearchResults(results));

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

  function edit(key: string, value: ConfigValue) { edits[key] = value; }
  function reset(key: string) { edits[key] = null; delete invalid[key]; }
  function undo(key: string) { delete edits[key]; delete invalid[key]; }
  function setInvalid(key: string, text: string | null) {
    if (text === null) delete invalid[key];
    else invalid[key] = text;
  }
  function removeUnknown(key: string) {
    if (has(config, key)) edits[key] = null;
    else delete edits[key];
  }
  async function discard() {
    if (!(await page.confirmDiscardChanges(pending.size))) return;
    edits = {};
    invalid = {};
    review = 'all';
  }

  function open(id: string, event?: MouseEvent) {
    if (event) {
      if (!page.isPlainClick(event)) return;
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
      const needsRestart = result.restart_required !== false;
      if (needsRestart) saved = true;
      else notify(saved ? 'Settings saved. Restart Rubylight for earlier changes to apply.' : 'Settings saved.', 'ok');
      if (result.warning) notify(result.warning, 'warn', 8000);
      void load();
    } catch (error) {
      failed('Saving settings failed', error);
    } finally {
      saving = false;
    }
  }

  async function restart() {
    if (!(await page.confirmRestartHost())) return;
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

  $effect(() => {
    if (pending.size === 0) return;
    return page.installUnsavedGuard(() => pending.size);
  });
</script>

{#snippet row(setting: Setting, inactive: boolean)}
  {@const model = page.buildRowModel(setting, defaults, edits, config, changes, invalid, values, { edit, reset, setInvalid })}
  {@const part = model.part}
  <SettingRow
    {setting}
    value={model.value}
    stored={model.stored}
    changed={model.changed}
    resetting={model.resetting}
    differs={model.differs}
    fixed={model.fixed}
    pnp={model.pnp}
    daily={model.daily}
    showKey={showKeys || (searching && matchesWords(setting.key, words))}
    {inactive}
    unavailable={inactive ? setting.hostGated?.(values) : undefined}
    reason={inactive ? setting.hostReason?.(values) : undefined}
    missing={missing(setting)}
    caution={caution(setting, values)}
    invalidText={invalid[setting.key]}
    onchange={(value) => edit(setting.key, value)}
    onreset={() => { reset(setting.key); if (part) reset(part.key); }}
    onundo={() => { undo(setting.key); if (part) undo(part.key); }}
    oninvalid={(text) => setInvalid(setting.key, text)}
  />
  {#if setting.key === PRESET}
    <PresetNote description={preset.description} problem={presetProblem} {handSet} />
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
  <UnknownKeys {keys} stored={config} {edits} {changes} {isDescribed} {adding} onedit={edit} onremove={removeUnknown} onundo={undo} />
{/snippet}

{#snippet viewLogs()}<Button size="sm" href="/logs" icon="logs">View logs</Button>{/snippet}

{#snippet group(item: Group)}
  {@const name = item.name || active.label}
  {@const anchor = groupAnchor(name)}
  {#if item.name === GAME_SOURCES}
    <GameSourcesPanel id={anchor} title={name} />
  {:else if active.id === 'pc' && item.name === 'Updates'}
    <!-- The update settings above the release status they act on, as on the Settings home. -->
    <UpdatesPanel id={anchor} title={name}>
      {#snippet settings()}{@render rows(item)}{/snippet}
    </UpdatesPanel>
  {:else if active.id === 'pc' && item.name === 'Sleep and wake'}
    <PowerPanel id={anchor} title={name}>
      {#snippet settings()}{@render rows(item)}{/snippet}
    </PowerPanel>
  {:else if TRAILING_GROUPS.has(item.name) || FOLDED_GROUPS.has(item.name)}
    {@const fold = foldStatus(item.settings, edits, config, pending)}
    <!-- Opened by the user. A customized value opens it until the user closes it; an unsaved one keeps it open. -->
    <Disclosure
      id={anchor}
      title={name}
      level={3}
      flush
      open={(foldOpen[item.name] ?? fold.customized > 0) || fold.mustOpen}
      ontoggle={(open) => (foldOpen[item.name] = open)}
    >
      {#snippet facts()}<DisclosureMeta count={item.settings.length} customized={fold.customized} unsaved={fold.unsaved} />{/snippet}
      {@render rows(item)}
    </Disclosure>
  {:else}
    <Panel id={anchor} title={name} level={3} flush actions={item.name === 'Logging' ? viewLogs : undefined}>
      {@render rows(item)}
    </Panel>
  {/if}
{/snippet}

<PageHeader title="All settings" subtitle="Every setting, by topic. Search finds any of them.">
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
    <SettingsSidebar
      bind:query
      {tabs}
      activeId={active.id}
      {browsingResults}
      {changedIn}
      onopen={open}
      onsearchinput={() => (review = 'all')}
    />

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
        <SettingsResults
          {review}
          {listingCustomized}
          pendingCount={pending.size}
          {invalidCount}
          {resultCount}
          {searching}
          {query}
          emptyTitle={review === 'invalid' ? 'All corrections complete' : review === 'changed' ? 'No unsaved changes' : listingCustomized ? 'All settings are at their defaults' : 'No settings match'}
          emptyMessage={review === 'invalid' ? 'You can save your changes now.' : review === 'changed' ? 'Your settings are up to date.' : listingCustomized ? 'Settings you change from their defaults appear here.' : 'Search looks at names, config keys, descriptions, topics and menu choices.'}
          {results}
          isVisible={visible}
          onclose={() => open(active.id)}
          onopentab={(id) => open(id)}
          {row}
          {otherKeys}
        />
      {:else}
        <h2 class="visually-hidden">{active.label}</h2>
        {#if sections.length > 1}
          <nav class="contents" aria-label="{active.label} sections" bind:clientHeight={contentsHeight}>
            <SettingsContentsTrack {sections} {current} onjump={jump} />
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

        {#if tabEmpty}
          <Panel><p class="muted">None of these settings apply with the current choices.</p></Panel>
        {/if}
        {#each tabGroups as item (item.name)}
          {@render group(item)}
        {/each}

        {#if debugGroup}
          {@render group(debugGroup)}
        {/if}

        {#if active.id === 'picture'}
          <!-- Installs and status, not settings keys. -->
          <Disclosure
            id={groupAnchor(INTEGRATIONS)}
            title={INTEGRATIONS}
            meta={nvidiaShown ? 'Vulkan HDR layer, RTSS and NVIDIA components' : 'Vulkan HDR layer and RTSS'}
            level={3}
            open={integrationsOpen}
            ontoggle={(open) => (integrationsOpen = open)}
          >
            <div class="integration-panels">
              <VulkanPanel />
              <LimiterPanel />
              {#if nvidiaShown}
                <NvidiaPanel />
              {/if}
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
  .content {
    display: grid;
    gap: var(--space-4);
    align-content: start;
    min-width: 0;
    scroll-margin-top: var(--space-4);
  }
  .content:focus { outline: none; }
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
    .pair { grid-template-columns: repeat(2, minmax(0, 1fr)); }
  }
  .load-error { align-items: center; justify-content: space-between; flex-wrap: wrap; }
  .placeholder { padding: var(--space-5); font-size: var(--text-sm); }
  .rows { display: grid; }
  .rows > :global(.setting + .setting) { border-top: 1px solid var(--line); }
  .integration-panels { display: grid; gap: var(--space-4); min-width: 0; }
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
    .layout { grid-template-columns: minmax(0, 1fr); gap: var(--space-4); }
  }
  @media (max-width: 520px) {
    .show-keys { min-height: var(--control-height); }
    .contents { margin-inline: calc(-1 * var(--space-4)); padding-inline: var(--space-4); }
  }
</style>
