<script lang="ts">
  import SettingControl from './SettingControl.svelte';
  import type { ConfigValue } from '../../lib/api';
  import type { Setting } from '../../lib/settings-types';
  import { describeDefault } from './values';

  let {
    setting,
    value,
    stored,
    changed,
    resetting,
    differs,
    showKey = false,
    inactive = false,
    missing = false,
    invalidText,
    onchange,
    onreset,
    onundo,
    oninvalid,
  }: {
    setting: Setting;
    /** The effective value, normalised for the control. */
    value: ConfigValue;
    /** The host has a value for this key. */
    stored: boolean;
    /** Saving would change what the host stores. */
    changed: boolean;
    /** Reset to default is waiting to be saved. */
    resetting: boolean;
    /** After saving, the host would use something other than the default. */
    differs: boolean;
    /** Show the configuration key (otherwise it is in the label's tooltip). */
    showKey?: boolean;
    /** Normally hidden by the current choices, but shown in search or while edited. */
    inactive?: boolean;
    /** Empty although the current choices need a value. */
    missing?: boolean;
    invalidText?: string;
    onchange: (value: ConfigValue) => void;
    onreset: () => void;
    onundo: () => void;
    oninvalid: (text: string | null) => void;
  } = $props();

  const uid = $props.id();
  const id = `${uid}-control`;
  const kind = $derived(setting.control.kind);
  /** Controls with several fields get a group named by the label instead of a label element. */
  const grouped = $derived(kind === 'list' || kind === 'commands' || kind === 'server-commands');
  /** Wide controls (and paths with their folder browser) go under the text instead of in the control column. */
  const full = $derived(grouped || kind === 'json' || (setting.control.kind === 'text' && !!setting.control.browse));
  /** An unsaved value or invalid draft. Either can be undone. */
  const marked = $derived(changed || invalidText !== undefined);
  const fallback = $derived(describeDefault(setting));
  /** Reset only when it does something Undo does not. */
  const canReset = $derived(differs && !resetting && (stored || !marked));
  const hasMeta = $derived(showKey || marked || resetting || differs);
</script>

<div
  class="setting"
  class:marked
  class:invalid={invalidText !== undefined}
  class:full
  class:switch-row={kind === 'toggle'}
  data-setting={setting.key}
>
  <div class="layout" class:has-meta={hasMeta}>
    <div class="text">
      <div class="head" title={showKey ? undefined : `Config key: ${setting.key}`}>
        {#if grouped}
          <span class="label" id="{uid}-label">{setting.label}</span>
        {:else}
          <label class="label" for={id}>{setting.label}</label>
        {/if}
        {#if setting.restart}<span class="tag">Applies after a restart</span>{/if}
      </div>
      {#if setting.description}<p class="description" id="{uid}-description">{setting.description}</p>{/if}
      {#if setting.details}
        <details class="more">
          <summary>More<span class="visually-hidden">{` about ${setting.label}`}</span></summary>
          <p>{setting.details}</p>
        </details>
      {/if}
      {#if inactive}
        <p class="dependency">{setting.inactiveHint ?? 'This setting is conditional. Review the related choices in this category.'}</p>
      {/if}
    </div>

    <!-- Not class={kind}: "select" would pick up the global form style. -->
    <div
      class="control"
      data-kind={kind}
      role={grouped ? 'group' : undefined}
      aria-labelledby={grouped ? `${uid}-label` : undefined}
    >
      <SettingControl
        {setting}
        {id}
        {value}
        {onchange}
        {invalidText}
        {oninvalid}
        describedby={setting.description ? `${uid}-description` : undefined}
      />
      {#if missing && invalidText === undefined}
        <p class="required">Required: streams fail until this has a value.</p>
      {/if}
    </div>

    {#if hasMeta}
      <div class="meta">
        {#if showKey}<code class="key">{setting.key}</code>{/if}
        {#if resetting}
          <span class="state">Resets to the default when saved</span>
        {:else if marked}
          <span class="state">{invalidText !== undefined ? 'Needs correction' : 'Unsaved change'}</span>
        {/if}
        {#if marked || resetting}
          <button type="button" class="link" onclick={onundo}>
            Undo<span class="visually-hidden">{` ${setting.label}`}</span>
          </button>
        {/if}
        {#if differs && !resetting}
          <span>Default: <span class="default" class:mono={fallback.literal}>{fallback.text}</span></span>
        {/if}
        {#if canReset}
          <button type="button" class="link" onclick={onreset}>
            Reset to default<span class="visually-hidden">{` for ${setting.label}`}</span>
          </button>
        {/if}
      </div>
    {/if}
  </div>
</div>

<style>
  .setting {
    container: setting / inline-size;
    min-width: 0;
  }
  .layout {
    /* Windows Settings: words on the left, the control in a fixed column on the right. */
    display: grid;
    grid-template-columns: minmax(0, 1fr) 320px;
    grid-template-areas: 'text control';
    align-items: start;
    gap: 6px var(--space-6);
    padding: 14px var(--space-5);
  }
  .has-meta {
    grid-template-rows: auto 1fr;
    grid-template-areas:
      'text control'
      'meta control';
  }
  .marked .layout {
    box-shadow: inset 3px 0 0 var(--accent);
  }
  .invalid .layout {
    box-shadow: inset 3px 0 0 var(--danger);
  }
  .text {
    grid-area: text;
    display: grid;
    gap: 3px;
    min-width: 0;
    align-content: start;
  }
  .head {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-1) var(--space-2);
    min-height: 22px;
  }
  .label {
    font-weight: var(--weight-medium);
    font-size: var(--text);
    line-height: 1.35;
  }
  label.label {
    cursor: pointer;
  }
  .tag {
    padding: 0 7px;
    border: 1px solid var(--line);
    border-radius: var(--radius-pill);
    font-size: var(--text-xs);
    line-height: 18px;
    color: var(--muted);
    white-space: nowrap;
  }
  .description {
    font-size: var(--text-sm);
    color: var(--muted);
    line-height: 1.45;
    max-width: 68ch;
  }
  .more {
    font-size: var(--text-sm);
    color: var(--muted);
    max-width: 68ch;
  }
  .more summary {
    display: inline-flex;
    align-items: center;
    gap: var(--space-1);
    width: fit-content;
    padding: 2px 0;
    color: var(--ink-2);
    font-size: var(--text-xs);
    font-weight: var(--weight-medium);
    cursor: pointer;
    list-style: none;
  }
  .more summary::-webkit-details-marker {
    display: none;
  }
  .more summary::after {
    content: '';
    width: 5px;
    height: 5px;
    margin-left: 2px;
    border-right: 1.5px solid currentColor;
    border-bottom: 1.5px solid currentColor;
    transform: translateY(-2px) rotate(45deg);
    transition: transform var(--duration-fast);
  }
  .more[open] summary::after {
    transform: translateY(1px) rotate(225deg);
  }
  .more summary:hover {
    color: var(--ink);
    text-decoration: underline;
    text-underline-offset: 2px;
  }
  .more p {
    margin-top: var(--space-1);
    padding-left: var(--space-3);
    border-left: 2px solid var(--line);
    line-height: var(--leading);
  }
  .dependency {
    margin-top: 2px;
    color: var(--ink-2);
    font-size: var(--text-xs);
    line-height: 1.45;
    padding-left: var(--space-2);
    border-left: 2px solid var(--line-strong);
  }
  .meta {
    grid-area: meta;
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 2px var(--space-3);
    min-width: 0;
    font-size: var(--text-xs);
    color: var(--muted);
  }
  .key {
    color: var(--ink-2);
    overflow-wrap: anywhere;
  }
  .state {
    color: var(--ink);
    font-weight: var(--weight-semibold);
  }
  .invalid .state {
    color: var(--danger);
  }
  .default {
    color: var(--ink-2);
    overflow-wrap: anywhere;
  }
  .link {
    border: 0;
    padding: 0;
    background: none;
    color: var(--ink-2);
    font-size: var(--text-xs);
    text-decoration: underline;
    text-underline-offset: 2px;
    cursor: pointer;
  }
  .link:hover {
    color: var(--ink);
  }
  .control {
    grid-area: control;
    display: grid;
    gap: 6px;
    min-width: 0;
  }
  .switch-row .control {
    justify-self: end;
    padding-top: 1px;
  }
  .required {
    font-size: var(--text-xs);
    line-height: 1.45;
    color: var(--warn);
    font-weight: var(--weight-medium);
  }
  /* Lists, commands and JSON: the control spans the row under the text. */
  .full .layout {
    grid-template-columns: minmax(0, 1fr);
    grid-template-rows: none;
    grid-template-areas:
      'text'
      'control';
    row-gap: var(--space-2);
  }
  .full .has-meta {
    grid-template-areas:
      'text'
      'control'
      'meta';
  }
  .full .control {
    max-width: 760px;
  }
  @container setting (max-width: 620px) {
    .layout {
      grid-template-columns: minmax(0, 1fr);
      grid-template-rows: none;
      grid-template-areas:
        'text'
        'control';
      gap: var(--space-2);
      padding: var(--space-3) var(--space-4);
    }
    .layout.has-meta {
      grid-template-areas:
        'text'
        'control'
        'meta';
    }
    /* A switch stays beside its label, as on a phone's own settings. */
    .switch-row .layout {
      grid-template-columns: minmax(0, 1fr) auto;
      grid-template-areas: 'text control';
      column-gap: var(--space-4);
    }
    .switch-row .layout.has-meta {
      grid-template-areas:
        'text control'
        'meta meta';
    }
  }
  @media (max-width: 520px) {
    .more summary,
    .link {
      min-height: var(--control-height);
    }
  }
</style>
