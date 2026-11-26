<script lang="ts">
  /**
   * Override cascade cards: one card per stream setting, showing which layer decides it
   * (game on device, game, device, host), each layer's own value, a control for the layer
   * this page edits and a "Reset to inherited" pill.
   *
   * The cards only write the first layer, through `onset`; the page saves it with the rest of
   * its form. Resolution is override-cascade.ts, the keys are the catalog's overridable ones.
   */
  import Badge from '../Badge.svelte';
  import Banner from '../Banner.svelte';
  import Button from '../Button.svelte';
  import Disclosure from '../Disclosure.svelte';
  import type { Setting } from '../../lib/settings-types';
  import {
    CASCADE_GROUPS,
    OWNER_TAG,
    cascadeSetting,
    controlText,
    isSet,
    rangeError,
    resolve,
    setCount,
    shown,
    storedValue,
    type CascadeGroup,
    type Layer,
  } from './override-cascade';

  let {
    id,
    layers,
    onset,
    groups = CASCADE_GROUPS,
    skip = [],
    hostError = '',
    onretry,
  }: {
    /** Unique on the page: the cards' controls take their ids from it. */
    id: string;
    /** Winner first: the layer these cards edit, then the ones below it, the host last. */
    layers: readonly Layer[];
    /** Set the first layer's value; `undefined` clears it, so the next layer decides. */
    onset: (key: string, value: string | number | boolean | undefined) => void;
    groups?: readonly CascadeGroup[];
    /** Keys the page already offers with a field of their own. */
    skip?: readonly string[];
    /** Why the host's settings could not be read; the cards still set overrides. */
    hostError?: string;
    onretry?: () => void;
  } = $props();

  const top = $derived(layers[0]);
  const below = $derived(layers.slice(1));
  const hostUnread = $derived(layers.some((layer) => layer.id === 'host' && layer.values === null));
  const folds = $derived(
    groups
      .map((group) => {
        const settings = group.keys
          .filter((key) => !skip.includes(key))
          .flatMap((key) => {
            const setting = cascadeSetting(key);
            return setting ? [setting] : [];
          });
        return { ...group, settings, own: setCount(settings.map((setting) => setting.key), top?.values) };
      })
      .filter((group) => group.settings.length > 0),
  );

  /** Folds the user opened or closed; one with a value of this layer starts open. */
  let opened = $state<Record<string, boolean>>({});
  /** Text being typed into a number or text field, kept as typed while it has the focus. */
  let typing = $state<Record<string, string>>({});

  function type(setting: Setting, text: string) {
    typing[setting.key] = text;
    if (!rangeError(setting, text)) onset(setting.key, storedValue(setting, text));
  }

  function inherit(setting: Setting): string {
    if (!below.length) return 'Not set';
    const next = resolve(setting, below);
    return next.unknown || !isSet(next.value) ? 'Inherit' : `Inherit: ${shown(setting, next.value)}`;
  }
</script>

<div class="cascade">
  {#if hostError}
    <Banner tone="warn" title="The host’s settings could not be read">
      Inherited values are not shown. Overrides can still be set. {hostError}
      {#snippet actions()}
        {#if onretry}<Button size="sm" icon="refresh" onclick={onretry}>Try again</Button>{/if}
      {/snippet}
    </Banner>
  {/if}
  {#each folds as fold (fold.id)}
    <Disclosure
      variant="inline"
      title={fold.title}
      meta={fold.own ? `${fold.settings.length} settings, ${fold.own} set here` : `${fold.settings.length} settings`}
      open={opened[fold.id] ?? fold.own > 0}
      ontoggle={(open) => (opened[fold.id] = open)}
    >
      <div class="cards">
        {#each fold.settings as setting (setting.key)}
          {@const resolved = resolve(setting, layers)}
          {@const own = top?.values?.[setting.key]}
          {@const text = typing[setting.key] ?? controlText(setting, own)}
          {@const control = setting.control}
          {@const field = `${id}-${setting.key}`}
          {@const error = rangeError(setting, text)}
          <article class="card" class:own={isSet(own)} aria-labelledby="{field}-label">
            <header>
              <h4 id="{field}-label">{setting.label}</h4>
              <Badge tone={resolved.owner === top?.id ? 'accent' : 'neutral'}>{OWNER_TAG[resolved.owner]}</Badge>
            </header>
            <p class="value" class:unread={resolved.unknown}>
              {resolved.unknown ? (hostError ? 'Not read' : 'Reading…') : shown(setting, resolved.value)}
            </p>
            <ol class="chain" aria-label="Where it comes from, the deciding one first">
              {#each resolved.steps as step (step.layer.id)}
                <li class:wins={step.wins} class:over={step.set && !step.wins}>
                  <span class="dot" aria-hidden="true"></span>
                  <span class="layer" title={step.layer.name}>{step.layer.name}</span>
                  <span class="at">
                    {#if !step.set}
                      Not set
                    {:else if step.layer.id === 'host' && hostUnread}
                      {hostError ? 'Not read' : 'Reading…'}
                    {:else}
                      {shown(setting, step.value)}
                    {/if}
                  </span>
                  {#if step.wins}<span class="visually-hidden">, decides</span>{/if}
                </li>
              {/each}
            </ol>
            <div class="edit">
              <label class="visually-hidden" for={field}>{setting.label} for {top?.name}</label>
              {#if control.kind === 'toggle' || control.kind === 'select'}
                <select
                  id={field}
                  class="select"
                  value={text}
                  onchange={(event) => onset(setting.key, storedValue(setting, event.currentTarget.value))}
                >
                  <option value="">{inherit(setting)}</option>
                  {#if control.kind === 'toggle'}
                    <option value="on">On</option>
                    <option value="off">Off</option>
                    {#if text && text !== 'on' && text !== 'off'}<option value={text}>{text} (not a choice)</option>{/if}
                  {:else}
                    {#if text && !control.options.some((option) => option.value === text)}
                      <option value={text}>{text} (not a choice)</option>
                    {/if}
                    {#each control.options as option (option.value)}
                      <option value={option.value}>{option.label}</option>
                    {/each}
                  {/if}
                </select>
              {:else}
                <span class="typed">
                  <input
                    id={field}
                    class="input"
                    inputmode={control.kind === 'number' ? 'decimal' : undefined}
                    autocomplete="off"
                    spellcheck="false"
                    placeholder={inherit(setting)}
                    list={control.kind === 'text' && control.presets ? `${field}-presets` : undefined}
                    aria-invalid={error ? 'true' : undefined}
                    aria-describedby={error ? `${field}-error` : undefined}
                    value={text}
                    oninput={(event) => type(setting, event.currentTarget.value)}
                    onblur={() => delete typing[setting.key]}
                  />
                  {#if control.kind === 'number' && control.unit}<span class="unit">{control.unit}</span>{/if}
                  {#if control.kind === 'text' && control.presets}
                    <datalist id="{field}-presets">
                      {#each control.presets as preset (preset.value)}
                        <option value={preset.value}>{preset.label}</option>
                      {/each}
                    </datalist>
                  {/if}
                </span>
              {/if}
              {#if isSet(own)}
                <button type="button" class="reset" onclick={() => { delete typing[setting.key]; onset(setting.key, undefined); }}>
                  Reset to inherited
                </button>
              {/if}
            </div>
            {#if error}<p class="error" id="{field}-error" role="alert">{error}</p>{/if}
          </article>
        {/each}
      </div>
    </Disclosure>
  {/each}
</div>

<style>
  .cascade {
    display: grid;
    gap: var(--space-2);
    min-width: 0;
  }
  .cards {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(min(100%, 272px), 1fr));
    gap: var(--space-3);
    min-width: 0;
  }
  .card {
    display: grid;
    align-content: start;
    gap: var(--space-3);
    padding: var(--space-4);
    border: 1px solid var(--line);
    border-radius: var(--radius-lg);
    background: var(--panel);
    box-shadow: var(--edge);
    min-width: 0;
    transition: border-color var(--duration-fast) var(--ease-standard);
  }
  .card.own {
    border-color: var(--accent-line);
  }
  header {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-1) var(--space-2);
    min-width: 0;
  }
  h4 {
    margin: 0;
    color: var(--ink-2);
    font-size: var(--text-sm);
    font-weight: var(--weight-medium);
  }
  .value {
    overflow-wrap: anywhere;
    color: var(--ink);
    font-family: var(--font-display);
    font-size: var(--text-lg);
    font-weight: var(--weight-bold);
    font-variant-numeric: tabular-nums;
    letter-spacing: var(--tracking-tight);
    line-height: var(--leading-tight);
  }
  .value.unread {
    color: var(--muted);
  }
  /* The chain: a rail of four stops, the deciding one filled. */
  .chain {
    position: relative;
    display: grid;
    gap: var(--space-1);
    margin: 0;
    padding: var(--space-3);
    list-style: none;
    border-radius: var(--radius-sm);
    background: var(--sunken);
  }
  .chain li {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr) auto;
    align-items: center;
    gap: var(--space-2);
    color: var(--muted);
    font-size: var(--text-xs);
    line-height: var(--leading-snug);
  }
  .dot {
    width: 8px;
    height: 8px;
    border: 1px solid var(--line-strong);
    border-radius: var(--radius-pill);
    background: var(--panel);
  }
  .layer {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .at {
    max-width: 22ch;
    overflow: hidden;
    text-align: end;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-variant-numeric: tabular-nums;
  }
  .chain .wins {
    color: var(--ink);
    font-weight: var(--weight-medium);
  }
  .chain .wins .dot {
    border-color: var(--accent);
    background: var(--accent);
  }
  .chain .over .at {
    text-decoration: line-through;
  }
  .chain .over .dot {
    border-color: var(--muted);
  }
  .edit {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-2);
    min-width: 0;
  }
  .edit .select,
  .typed {
    flex: 1 1 150px;
    min-width: 0;
  }
  .typed {
    display: flex;
    align-items: center;
    gap: var(--space-2);
  }
  .typed .input {
    flex: 1 1 auto;
    min-width: 72px;
    width: 100%;
    font-variant-numeric: tabular-nums;
  }
  .unit {
    color: var(--muted);
    font-size: var(--text-sm);
    white-space: nowrap;
  }
  .reset {
    min-height: var(--control-height);
    padding: 0 var(--space-3);
    border: 1px solid var(--accent-line);
    border-radius: var(--radius-pill);
    background: var(--accent-soft);
    color: var(--accent-text);
    font-size: var(--text-xs);
    font-weight: var(--weight-medium);
    white-space: nowrap;
    cursor: pointer;
    transition: background-color var(--duration-fast) var(--ease-standard);
  }
  .reset:hover {
    background: color-mix(in oklab, var(--accent-soft), var(--accent) 12%);
  }
  .error {
    color: var(--danger);
    font-size: var(--text-xs);
    line-height: var(--leading-snug);
  }
</style>
