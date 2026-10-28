<script lang="ts">
  import { onMount, tick } from 'svelte';
  import Button from '../components/Button.svelte';
  import EmptyState from '../components/EmptyState.svelte';
  import Icon from '../components/Icon.svelte';
  import PageHeader from '../components/PageHeader.svelte';
  import Panel from '../components/Panel.svelte';
  import SettingRow from '../components/settings/SettingRow.svelte';
  import Switch from '../components/settings/Switch.svelte';
  import UnknownKeys from '../components/settings/UnknownKeys.svelte';
  import LimiterPanel from '../components/maintenance/LimiterPanel.svelte';
  import VulkanPanel from '../components/maintenance/VulkanPanel.svelte';
  import { matchesSetting, matchesWords, searchWords } from '../components/settings/search';
  import {
    applyChanges,
    computeChanges,
    currentValues,
    differsFromDefault,
    foldStatus,
    isSet,
    effective,
    has,
    normalise,
    NOT_SETTINGS,
    sameValue,
    settingsOnly,
    type Edits,
  } from '../components/settings/values';
  import { api, type Config, type ConfigValue } from '../lib/api';
  import { confirm, failed, notify } from '../lib/feedback.svelte';
  import { guardNavigation, match, navigate, query as queryParam, route } from '../lib/router.svelte';
  import { refreshMetadata } from '../lib/session.svelte';
  import { categoriesOf, schema, settingGroups, settingsGroup } from '../lib/settings-schema';
  import { DEBUG_GROUP, EXPERT_GROUPS } from '../lib/schema/main';
  import SaveBar from '../components/settings/SaveBar.svelte';
  import { SETTING as PRESET, covered, presetById, presetId, presetNotice, withPreset } from '../lib/stream-presets';
  import type { Category, CategoryId, Setting } from '../lib/settings-types';

  const categories = settingGroups;
  const byKey: ReadonlyMap<string, Setting> = new Map(schema.settings.map((setting) => [setting.key, setting]));
  const isDescribed = (key: string) => byKey.has(key);

  const section = $derived(match('/settings/:section')?.section ?? 'general');
  const active = $derived<Category | undefined>(
    categories.find((category) => category.id === settingsGroup(section)) ??
      categories.find((category) => category.id === 'general') ??
      categories[0],
  );

  /** What the host has stored; null until loaded. */
  let stored = $state<Config | null>(null);
  let loadError = $state('');
  let edits = $state<Edits>({});
  /** Invalid number or JSON drafts by key. Saving waits for correction. */
  let invalid = $state<Record<string, string>>({});
  // Search from the Settings home arrives as ?q=.
  let query = $state(queryParam('q') ?? '');
  let review = $state<'all' | 'changed' | 'invalid'>('all');
  // Keep reviewed rows mounted while they are corrected or reverted.
  let reviewKeys = $state<string[]>([]);
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
  const values = $derived(currentValues(described, edits, config));
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
    const counts: Partial<Record<CategoryId, number>> = {};
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

  /** Expert groups after the others, Debug switches last. */
  const rank = (name: string) => (name === DEBUG_GROUP ? 2 : EXPERT_GROUPS.has(name) ? 1 : 0);

  /** Settings under their group headings, groups in order of first use, expert groups last. */
  function grouped(list: Setting[]): Group[] {
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
    return groups.sort((a, b) => rank(a.name) - rank(b.name));
  }

  const inCategory = $derived(described.filter((setting) => settingsGroup(setting.category) === active?.id));
  // Keep a pending control reachable if editing its parent makes it inactive.
  const shown = $derived(inCategory.filter((setting) => visible(setting) || pending.has(setting.key)));
  const shownGroups = $derived(grouped(shown));

  // A link to one setting (?setting=key, from the Settings home's search)
  // opens its group, then brings the row into view and marks it.
  // A setting the current choices hide is shown through search instead.
  let jumped = '';
  $effect(() => {
    const key = new URLSearchParams(route.search).get('setting');
    if (!key || stored === null || jumped === route.path + route.search) return;
    jumped = route.path + route.search;
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
      if (mark) row.animate([{ backgroundColor: mark }, { backgroundColor: 'transparent' }], { duration: 1800, easing: 'ease-out' });
    });
  });

  // ---------- search ----------

  const words = $derived(searchWords(query));
  const searching = $derived(words.length > 0);
  const browsingResults = $derived(searching || review !== 'all');
  const inReview = (key: string) => review === 'all' || reviewKeys.includes(key);

  const results = $derived.by(() => {
    if (!browsingResults) return [];
    return categories
      .map((category) => ({
        category,
        settings: described.filter(
          (setting) =>
            settingsGroup(setting.category) === category.id &&
            inReview(setting.key) && categoriesOf(setting).some((shownUnder) => matchesSetting(setting, shownUnder, words)),
        ),
        unknown: category.id === 'general' ? unknownKeys.filter((key) => inReview(key) && matchesWords(key, words)) : [],
      }))
      .filter((result) => result.settings.length > 0 || result.unknown.length > 0);
  });
  const resultCount = $derived(new Set(results.flatMap((result) => [
    ...result.settings.map((setting) => setting.key), ...result.unknown,
  ])).size);

  async function reviewChanges(mode: 'changed' | 'invalid') {
    query = '';
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
      message: `${count} unsaved ${count === 1 ? 'change is' : 'changes are'} lost. The host keeps its current settings.`,
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
      message: 'Streams in progress disconnect. This console reconnects by itself when the host is back.',
      confirm: 'Restart',
    });
    if (!ok) return;
    restarting = true;
    try {
      await api.host.restart();
      saved = false;
      notify('Rubylight is restarting. This console reconnects when the host is back.', 'info', 8000);
    } catch (error) {
      failed('Restart failed', error);
    } finally {
      restarting = false;
    }
  }

  // Leaving the page with unsaved changes asks first; switching categories
  // keeps the edits.
  $effect(() => {
    if (pending.size === 0) return;
    const warn = (event: BeforeUnloadEvent) => event.preventDefault();
    window.addEventListener('beforeunload', warn);
    const unguard = guardNavigation((to) =>
      // Other categories keep the edits; the Settings home is another page.
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
    missing={missing(setting)}
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

{#snippet groups(list: Group[])}
  {#each list as group (group.name)}
    {#if EXPERT_GROUPS.has(group.name)}
      {@const fold = foldStatus(group.settings, edits, config, pending)}
      <!-- Opened by the user. A customized value opens it until the user closes it; an unsaved one keeps it open. -->
      <details
        class="group fold"
        open={(foldOpen[group.name] ?? fold.customized > 0) || fold.mustOpen}
        ontoggle={(event) => {
          foldOpen[group.name] = event.currentTarget.open;
        }}
      >
        <summary>
          <Icon name="chevron" size={15} />
          {group.name}
          <span class="count">
            {group.settings.length} {group.settings.length === 1 ? 'setting' : 'settings'}{fold.customized ? `, ${fold.customized} customized` : ''}{fold.unsaved ? `, ${fold.unsaved} unsaved` : ''}
          </span>
        </summary>
        {#each group.settings as setting (setting)}
          {@render row(setting, !visible(setting))}
        {/each}
      </details>
    {:else}
      <section class="group">
        {#if group.name}<h3 class="group-title">{group.name}</h3>{/if}
        <!-- Keyed by object: a key may be listed in two categories. -->
        {#each group.settings as setting (setting)}
          {@render row(setting, !visible(setting))}
        {/each}
      </section>
    {/if}
  {/each}
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

<PageHeader title="All settings" subtitle="Every setting this host reads, by topic. The few most people change are on the Settings page.">
  {#snippet actions()}
    <label class="show-keys">
      <span>Show config keys</span>
      <Switch checked={showKeys} onchange={setShowKeys} />
    </label>
  {/snippet}
</PageHeader>

<div class="frame">
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
        <span>Settings group</span>
        <select aria-label="Settings group" class="select" onchange={(event) => open(event.currentTarget.value)}>
          {#if browsingResults}<option value="" disabled selected>All categories</option>{/if}
          {#each categories as category (category.id)}
            {@const count = changedIn[category.id] ?? 0}
            <option value={category.id} selected={!browsingResults && category.id === active?.id}>
              {category.label}{count ? ` (${count} unsaved)` : ''}
            </option>
          {/each}
        </select>
      </label>

      <nav aria-label="Settings categories">
        <ul>
          {#each categories as category (category.id)}
            {@const count = changedIn[category.id] ?? 0}
            <li>
              <a
                href="/settings/{category.id}"
                aria-current={!browsingResults && category.id === active?.id ? 'page' : undefined}
                onclick={(event) => open(category.id, event)}
              >
                <span class="name">
                  {category.label}
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
            <h2>{review === 'invalid' ? 'Review errors' : review === 'changed' ? 'Review changes' : 'Search results'}</h2>
            <p class="muted" role="status">
              {#if review !== 'all' && pending.size === 0}
                No unsaved changes.
              {:else if review === 'invalid' && invalidCount === 0}
                All errors corrected. Review your values, then save your changes.
              {:else}
                {resultCount} {resultCount === 1 ? 'setting' : 'settings'}{searching ? ` matching “${query.trim()}”` : ' across all categories'}
              {/if}
            </p>
          </div>
          <Button size="sm" variant="ghost" onclick={() => open(active?.id ?? 'general')}>All settings</Button>
        </div>
        {#if results.length === 0}
          <Panel>
            <EmptyState icon="search" title={review === 'invalid' ? 'All corrections complete' : review === 'changed' ? 'No unsaved changes' : 'No settings match'}>
              <p>{review === 'invalid' ? 'You can save your changes now.' : review === 'changed' ? 'Your settings are up to date.' : 'Search looks at names, config keys, descriptions, categories and menu choices.'}</p>
            </EmptyState>
          </Panel>
        {:else}
          {#each results as result (result.category.id)}
            <Panel title={result.category.label} flush>
              {#snippet actions()}
                <Button size="sm" variant="ghost" onclick={() => open(result.category.id)}>
                  Open<span class="visually-hidden">{` ${result.category.label}`}</span>
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
      {:else if active}
        <Panel title={active.label} description={active.description} flush>
          {#if inCategory.length === 0}
            {#if active.id !== 'advanced'}
              <EmptyState icon="settings" title="No settings in this category">
                <p>There are no {active.label.toLowerCase()} settings available.</p>
              </EmptyState>
            {/if}
          {:else}
            <div class="rows">
              {@render groups(shownGroups)}
              {#if shown.length === 0}
                <p class="placeholder muted">None of these settings apply with the current choices.</p>
              {/if}
            </div>
          {/if}
          {#if active.id === 'display'}
            <!-- Collapsed like the expert groups: installs and status, not settings keys. -->
            <details class="fold integrations">
              <summary><Icon name="chevron" size={15} />Integrations <span class="count">Vulkan HDR layer and RTSS</span></summary>
              <div class="integration-panels">
                <VulkanPanel />
                <LimiterPanel />
              </div>
            </details>
          {/if}
          {#if active.id === 'general'}
            <details class="fold custom-config" open={section === 'advanced'}>
              <summary><Icon name="chevron" size={15} />Custom configuration keys <span class="count">{unknownKeys.length} stored</span></summary>
              {@render otherKeys(unknownKeys, true)}
            </details>
          {/if}
        </Panel>
        {#if active.id === 'network'}<div class="row"><Button href="/tokens" icon="key">API tokens</Button><Button href="/webhooks" icon="link">Webhooks</Button><Button href="/settings#security">Sign-in and security</Button></div>{/if}
        {#if active.id === 'general'}<div class="row"><Button href="/settings#updates">Updates</Button><Button href="/logs" icon="logs">View logs</Button></div>{/if}
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
  .custom-config { border-top: 1px solid var(--line); }
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
  .search {
    max-width: 440px;
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
  nav ul {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-1);
    margin: 0;
    padding: 0;
    list-style: none;
  }
  nav a {
    display: grid;
    gap: 1px;
    padding: var(--space-3) 14px;
    min-height: var(--control-height);
    border-radius: var(--radius);
    color: var(--ink-2);
    text-decoration: none;
  }
  nav a:hover {
    background: var(--panel);
    color: var(--ink);
  }
  nav a[aria-current='page'] {
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
  /* Section headings with hairlines between the rows under them. */
  .group-title {
    padding: var(--space-5) var(--space-5) var(--space-2);
    font-size: var(--text);
    font-weight: var(--weight-semibold);
    color: var(--ink);
  }
  .group:first-child .group-title {
    padding-top: var(--space-4);
  }
  .group-title + :global(.setting),
  .group :global(.setting + .setting),
  .rows > :global(.setting + .setting) {
    border-top: 1px solid var(--line);
  }
  .group + .group:not(:has(.group-title)) {
    border-top: 1px solid var(--line);
  }
  .count {
    font-weight: var(--weight-regular);
    font-size: var(--text-sm);
    color: var(--muted);
  }
  .fold {
    border-top: 1px solid var(--line);
  }
  .fold > summary {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    padding: var(--space-4) var(--space-5);
    font-size: var(--text);
    font-weight: var(--weight-semibold);
    cursor: pointer;
    list-style: none;
  }
  .fold > summary::-webkit-details-marker {
    display: none;
  }
  .fold > summary:hover {
    background: var(--sunken);
  }
  .fold > summary :global(svg) {
    color: var(--muted);
    transition: transform var(--duration-fast);
  }
  .fold[open] > summary :global(svg) {
    transform: rotate(90deg);
  }
  .fold > :global(.setting) {
    border-top: 1px solid var(--line);
  }
  .integration-panels {
    display: grid;
    gap: var(--space-4);
    padding: 0 var(--space-5) var(--space-5);
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
    nav {
      display: none;
    }
  }
  @media (max-width: 520px) {
    .side {
      grid-template-columns: minmax(0, 1fr);
    }
    .group-title,
    .integration-panels,
    .fold > summary {
      padding-left: var(--space-4);
      padding-right: var(--space-4);
    }
    .show-keys,
    .fold > summary {
      min-height: var(--control-height);
    }
  }
</style>
