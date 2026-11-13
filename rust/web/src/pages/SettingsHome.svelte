<script lang="ts">
  import { onMount } from 'svelte';
  import Banner from '../components/Banner.svelte';
  import Button from '../components/Button.svelte';
  import Icon from '../components/Icon.svelte';
  import PageHeader from '../components/PageHeader.svelte';
  import Panel from '../components/Panel.svelte';
  import Section from '../components/maintenance/Section.svelte';
  import UpdatesPanel from '../components/maintenance/UpdatesPanel.svelte';
  import { reason } from '../components/maintenance/util';
  import SaveBar from '../components/settings/SaveBar.svelte';
  import SettingRow from '../components/settings/SettingRow.svelte';
  import WakeRow from '../components/settings/WakeRow.svelte';
  import { matchesWords, searchSettings, searchWords } from '../components/settings/search';
  import {
    applyChanges,
    cautionPairs,
    cautionsPending,
    computeChanges,
    currentValues,
    differsFromDefault,
    dismissCautions,
    effective,
    handSetCautions,
    has,
    nativeEncoders,
    normalise,
    nvidiaEnhancements,
    sameValue,
    settingsOnly,
    type Edits,
  } from '../components/settings/values';
  import { api, type Config, type ConfigValue } from '../lib/api';
  import { confirm, failed, notify } from '../lib/feedback.svelte';
  import { guardNavigation, link, navigate, route } from '../lib/router.svelte';
  import { MAIN_SECTIONS, UPDATE_KEYS, onHome } from '../lib/schema/main';
  import { categoriesOf, groupAnchor, schema, settingGroups, settingsGroup } from '../lib/settings-schema';
  import { session as host } from '../lib/session.svelte';
  import { NATIVE_ENCODERS, NVIDIA_ENHANCEMENTS, type Setting } from '../lib/settings-types';
  import { SETTING as PRESET, covered, presetById, presetId, presetNotice, withPreset } from '../lib/stream-presets';

  const RESULTS = 8;

  let stored = $state<Config | null>(null);
  let loadError = $state('');
  let edits = $state<Edits>({});
  let invalid = $state<Record<string, string>>({});
  let query = $state('');
  let saving = $state(false);
  let saved = $state(false);
  let restarting = $state(false);
  let latestLoad = 0;

  const config = $derived(stored ?? {});
  const preset = $derived(presetById(presetId(has(edits, PRESET) ? edits[PRESET] : config[PRESET])));
  const presetProblem = $derived(presetNotice(has(edits, PRESET) ? edits[PRESET] : config[PRESET]));
  const described = $derived(withPreset(schema.settings, preset.id));
  const defaults: ReadonlyMap<string, Setting> = $derived(new Map(described.map((setting) => [setting.key, setting])));
  const values = $derived(currentValues(described, edits, config));
  const changes = $derived(stored ? computeChanges(defaults, edits, stored, new Set(covered)) : {});
  const pending = $derived(new Set([...Object.keys(changes), ...Object.keys(invalid)]));
  const invalidCount = $derived(Object.keys(invalid).length);

  // ---------- settings set by hand to a value that costs the stream ----------

  const cautions = $derived(
    stored === null
      ? []
      : handSetCautions(
          described,
          {
            ...values,
            // AMF settings apply only where Automatic would use AMF.
            [NATIVE_ENCODERS]: nativeEncoders(
              host.metadata?.gpus,
              String(values.adapter_name ?? ''),
              String(values.adapter_pnp_id ?? ''),
            ),
            [NVIDIA_ENHANCEMENTS]: nvidiaEnhancements(host.metadata?.features),
          },
          edits,
          config,
        ),
  );
  // Dismissing remembers the costly values shown; only a new one shows the banner again.
  const CAUTIONS_DISMISSED = 'rubylight.settings.cautions-dismissed';
  const pairs = $derived(cautionPairs(cautions, values));
  let dismissed = $state<string[]>(readDismissed());
  function readDismissed(): string[] {
    // A browser that blocks site data throws on localStorage: dismissing then lasts for this visit.
    try {
      const saved: unknown = JSON.parse(localStorage.getItem(CAUTIONS_DISMISSED) ?? '[]');
      return Array.isArray(saved) ? saved.filter((pair): pair is string => typeof pair === 'string') : [];
    } catch {
      return [];
    }
  }
  function dismiss() {
    dismissed = dismissCautions(pairs, dismissed);
    try {
      localStorage.setItem(CAUTIONS_DISMISSED, JSON.stringify(dismissed));
    } catch {
      // Kept for this visit only.
    }
  }

  // Monitors only turn off around a virtual display.
  const applies = (key: string) => key !== 'virtual_display_layout' || values.virtual_display_mode !== 'disabled' || pending.has(key);

  const sections = $derived(
    MAIN_SECTIONS.map((section) => ({
      ...section,
      settings: section.keys
        .map((key) => defaults.get(key))
        .filter((setting): setting is Setting => !!setting && applies(setting.key))
        .map((setting) => onHome(setting, normalise(setting, effective(setting, edits, config)))),
    })),
  );

  // ---------- search: every key, opened in All settings ----------

  // Settings named after the query first, then the rest in schema order.
  const results = $derived.by(() => {
    const words = searchWords(query);
    const named = (setting: Setting) => (matchesWords(setting.label, words) ? 0 : 1);
    return searchSettings(schema.settings, categoriesOf, query).sort((a, b) => named(a) - named(b));
  });
  const groupLabel = (setting: Setting) => settingGroups.find((group) => group.id === settingsGroup(setting.category))?.label ?? '';
  const target = (setting: Setting) =>
    `/settings/${settingsGroup(setting.category)}?setting=${encodeURIComponent(setting.key)}`;

  /** Where a tab's group is in All settings, such as This PC › Sleep and wake. */
  const inAllSettings = (tab: string, group: string) => `/settings/${tab}#${groupAnchor(group)}`;

  // Sign-in and security moved to All settings › Advanced; older links to it go there.
  const MOVED: Record<string, string> = { security: inAllSettings('advanced', 'Sign-in and security') };

  // ---------- loading and saving: the same PATCH of changed keys as All settings ----------

  async function load() {
    const id = ++latestLoad;
    try {
      const next = settingsOnly(await api.config.get());
      if (id !== latestLoad) return;
      stored = next;
      loadError = '';
    } catch (error) {
      if (id === latestLoad) loadError = reason(error);
    }
  }

  onMount(() => {
    const moved = MOVED[location.hash.slice(1)];
    if (moved) {
      navigate(moved, { replace: true });
      return;
    }
    void load();
  });

  // The page fills in after the first load, so a link to a section scrolls once it is there.
  let scrolled = false;
  $effect(() => {
    if (stored === null || scrolled) return;
    scrolled = true;
    const id = location.hash.slice(1);
    if (id) queueMicrotask(() => document.getElementById(id)?.scrollIntoView());
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
  }

  async function save() {
    const base = stored;
    if (!base || saving || pending.size === 0 || invalidCount > 0) return;
    const body = $state.snapshot(changes) as Config;
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
      notify('Rubylight is restarting. This console reconnects when Rubylight is back.', 'info', 8000);
    } catch (error) {
      failed('Restart failed', error);
    } finally {
      restarting = false;
    }
  }

  // Unsaved changes live on this page only, so leaving it asks first.
  $effect(() => {
    if (pending.size === 0) return;
    const warn = (event: BeforeUnloadEvent) => event.preventDefault();
    window.addEventListener('beforeunload', warn);
    const unguard = guardNavigation((to) =>
      to.split(/[?#]/)[0] === route.path
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

{#snippet row(setting: Setting)}
  <SettingRow
    {setting}
    value={normalise(setting, effective(setting, edits, config))}
    stored={has(config, setting.key)}
    changed={has(changes, setting.key)}
    resetting={edits[setting.key] === null && has(config, setting.key)}
    differs={differsFromDefault(setting, edits, config)}
    invalidText={invalid[setting.key]}
    onchange={(value) => edit(setting.key, value)}
    onreset={() => reset(setting.key)}
    onundo={() => undo(setting.key)}
    oninvalid={(text) => setInvalid(setting.key, text)}
  />
  {#if setting.key === PRESET}
    {#if presetProblem}
      <p class="note error" role="alert"><strong>Not a preset.</strong> {presetProblem}</p>
    {/if}
    <p class="note">{preset.description}</p>
  {/if}
{/snippet}

{#snippet rows(section: (typeof sections)[number])}
  <Panel flush>
    {#if stored === null}
      {#if !loadError}<p class="placeholder muted">Loading settings…</p>{/if}
    {:else}
      <div class="rows">
        {#each section.settings.filter((setting) => !UPDATE_KEYS.includes(setting.key)) as setting (setting.key)}{@render row(setting)}{/each}
        {#if section.id === 'pc'}<WakeRow details={inAllSettings('pc', 'Sleep and wake')} />{/if}
      </div>
    {/if}
    {#if section.id === 'games'}
      <p class="aside">
        Steam and your other launchers add their installed games by themselves.
        <a href="/library" use:link>Choose sources in Games</a>
      </p>
    {/if}
  </Panel>
{/snippet}

<div class="page">
  <PageHeader title="Settings" subtitle="The few things most people change.">
    {#snippet actions()}<Button href="/settings/pc" icon="maintenance">All settings</Button>{/snippet}
  </PageHeader>

  <div class="finder">
    <div class="search">
      <Icon name="search" size={16} />
      <input
        class="input"
        type="search"
        placeholder="Search all settings"
        aria-label="Search all settings"
        aria-controls="setting-results"
        autocomplete="off"
        spellcheck="false"
        bind:value={query}
      />
    </div>
    {#if query.trim()}
      <div id="setting-results" class="results" role="region" aria-label="Matching settings">
        {#if results.length === 0}
          <p class="empty" role="status">No setting matches “{query.trim()}”. Search looks at names, config keys, descriptions and menu choices.</p>
        {:else}
          <p class="count" role="status">{results.length} {results.length === 1 ? 'setting' : 'settings'}</p>
          <ul>
            {#each results.slice(0, RESULTS) as setting (setting.key)}
              <li>
                <a href={target(setting)} use:link>
                  <span class="result-label">{setting.label}</span>
                  <span class="where">{groupLabel(setting)}{setting.group ? ` › ${setting.group}` : ''}</span>
                </a>
              </li>
            {/each}
          </ul>
          {#if results.length > RESULTS}
            <a class="more" href="/settings/pc?q={encodeURIComponent(query.trim())}" use:link>
              Show all {results.length} in All settings<Icon name="chevron" size={14} />
            </a>
          {/if}
        {/if}
      </div>
    {/if}
  </div>

  {#if loadError}
    <div class="notice danger load-error" role="alert">
      <span>{stored ? 'Could not refresh the settings' : 'Could not load the settings'}: {loadError}</span>
      <Button size="sm" onclick={load}>Retry</Button>
    </div>
  {/if}

  {#if cautionsPending(pairs, dismissed)}
    <div class="cautions" data-cautions>
      <Banner
        tone="warn"
        title={cautions.length === 1 ? 'A changed setting has a cost or a risk' : `${cautions.length} changed settings have a cost or a risk`}
        ondismiss={dismiss}
      >
        <ul>
          {#each cautions as { setting, text } (setting.key)}
            <li><a href={target(setting)} use:link>{setting.label}</a>: {text}</li>
          {/each}
        </ul>
      </Banner>
    </div>
  {/if}

  {#each sections as section (section.id)}
    {@const tab = settingGroups.find((group) => group.id === section.id)}
    <div id={section.id} class="anchor">
      <Section title={section.title} more={tab && { href: `/settings/${tab.id}`, label: 'More settings' }}>
        {@render rows(section)}
        {#if section.id === 'pc'}
          <!-- The switch above the release status it acts on. -->
          <UpdatesPanel id="updates" more={inAllSettings('pc', 'Updates')}>
            {#snippet settings()}
              {#if stored}
                <div class="rows">
                  {#each section.settings.filter((setting) => UPDATE_KEYS.includes(setting.key)) as setting (setting.key)}{@render row(setting)}{/each}
                </div>
              {/if}
            {/snippet}
          </UpdatesPanel>
        {/if}
      </Section>
    </div>
  {/each}

  {#if stored}
    <SaveBar
      pending={pending.size}
      invalid={invalidCount}
      {saving}
      {saved}
      {restarting}
      ondiscard={discard}
      onsave={save}
      onrestart={restart}
      ondismiss={() => (saved = false)}
    />
  {/if}
</div>

<style>
  .page {
    display: grid;
    gap: var(--space-6);
    min-width: 0;
  }
  .anchor {
    scroll-margin-top: var(--space-4);
    min-width: 0;
  }
  .rows {
    display: grid;
  }
  .rows > :global(.setting + .setting),
  .rows > .note + :global(.setting) {
    border-top: 1px solid var(--line);
  }
  .note {
    margin: 0;
    padding: 0 var(--space-5) var(--space-3);
    color: var(--muted);
    font-size: var(--text-sm);
  }
  .note.error,
  .note.error strong {
    color: var(--danger);
  }
  .cautions ul {
    margin: 0;
    padding-left: var(--space-4);
    display: grid;
    gap: var(--space-1);
  }
  .aside {
    margin: 0;
    padding: var(--space-3) var(--space-5) var(--space-4);
    border-top: 1px solid var(--line);
    color: var(--muted);
    font-size: var(--text-sm);
  }
  .placeholder {
    padding: var(--space-5);
    font-size: var(--text-sm);
  }
  .load-error {
    align-items: center;
    justify-content: space-between;
    flex-wrap: wrap;
  }
  .finder {
    display: grid;
    gap: var(--space-2);
    max-width: 640px;
    margin-top: calc(-1 * var(--space-2));
  }
  .search {
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
  .results {
    display: grid;
    border: 1px solid var(--line);
    border-radius: var(--radius-lg);
    background: var(--panel);
    box-shadow: var(--shadow);
    overflow: hidden;
  }
  .results ul {
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .results li + li {
    border-top: 1px solid var(--line);
  }
  .results a {
    display: grid;
    gap: 2px;
    padding: var(--space-3) var(--space-4);
    min-height: var(--control-height);
    color: var(--ink);
    text-decoration: none;
  }
  .results a:hover,
  .results a:focus-visible {
    background: var(--sunken);
  }
  .result-label {
    font-weight: var(--weight-medium);
  }
  .where {
    font-size: var(--text-sm);
    color: var(--muted);
  }
  .count,
  .empty {
    margin: 0;
    padding: var(--space-3) var(--space-4);
    font-size: var(--text-sm);
    color: var(--muted);
  }
  .count {
    padding-bottom: var(--space-1);
  }
  .results .more {
    display: flex;
    align-items: center;
    gap: var(--space-1);
    border-top: 1px solid var(--line);
    font-size: var(--text-sm);
    font-weight: var(--weight-medium);
    color: var(--accent-text, var(--ink));
  }
  @media (max-width: 520px) {
    .note,
    .aside {
      padding-left: var(--space-4);
      padding-right: var(--space-4);
    }
  }
</style>
