<script lang="ts">
  import { onMount } from 'svelte';
  import Button from '../Button.svelte';
  import Field from '../Field.svelte';
  import Icon from '../Icon.svelte';
  import Toggle from '../Toggle.svelte';
  import { api, type RadeonPanel } from '../../lib/api';
  import type { AppDraft } from './draft.svelte';
  import {
    PROFILE_KEY,
    addRow,
    changeControls,
    clampTo,
    controlsOf,
    currentOf,
    effectiveSettings,
    formatValue,
    offered,
    pickProfile,
    profileOf,
    profiles,
    rawRows,
    rowError,
    supportOf,
    type AntiAliasing,
    type Controls,
    type FrameCap,
    type RadeonChoice,
  } from './radeon';

  let { draft }: { draft: AppDraft } = $props();

  const uid = $props.id();
  const KEY = PROFILE_KEY as 'radeon-profile';

  // What this PC's GPU supports. It is read once; the controls only appear for what it lists.
  let panel = $state<RadeonPanel | null>(null);
  let readError = $state('');
  let loading = $state(true);
  let rawOpen = $state(false);
  onMount(() => {
    api
      .radeon()
      .then((reading) => (panel = reading))
      .catch((error: unknown) => (readError = error instanceof Error ? error.message : String(error)))
      .finally(() => {
        loading = false;
        // Without controls the raw rows are the only way to list settings.
        if (!offered(supportOf(panel)).size) rawOpen = true;
      });
  });

  const support = $derived(supportOf(panel));
  const shown = $derived(offered(support));
  const stored = $derived(draft.text(KEY));
  const chosen = $derived(profileOf(stored));
  const custom = $derived(chosen === 'custom');
  const info = $derived(profiles.find((profile) => profile.id === chosen));
  // A profile this console does not know is kept as written and the host says what it thinks.
  const unknown = $derived(stored !== '' && !info);
  const controls = $derived(controlsOf(effectiveSettings(stored, draft.radeon), support).controls);
  const raw = $derived(rawRows(stored, draft.radeon, support));
  const reason = $derived(
    loading
      ? ''
      : readError
        ? `The Radeon settings could not be read: ${readError}`
        : shown.size
          ? ''
          : (panel?.message ?? "AMD's driver reports none of these controls as changeable on this GPU."),
  );

  const setting = (id: string) => support.get(id);
  const choices = (id: string) => (setting(id)?.options ?? []).filter((option) => option.value !== 'invalid');

  function apply(choice: RadeonChoice) {
    draft.setText(KEY, choice.profile);
    draft.radeon = choice.rows;
  }
  function change(patch: Partial<Controls>) {
    apply(changeControls(stored, draft.radeon, support, { ...controls, ...patch }));
  }
  function remove(row: (typeof draft.radeon)[number]) {
    const index = draft.radeon.indexOf(row);
    if (index >= 0) draft.radeon.splice(index, 1);
  }
</script>

{#snippet number(id: string, value: number | null, label: string, set: (value: number | null) => void)}
  {@const known = setting(id)}
  {#if known}
    <span class="number">
      <input
        class="input num"
        type="number"
        aria-label={label}
        min={known.range?.min}
        max={known.range?.max}
        step={known.range?.step || 1}
        placeholder={formatValue(known.value)}
        value={value ?? ''}
        onchange={(event) => {
          const next = clampTo(known, event.currentTarget.value);
          // Shows the clamped number even when it equals the old one.
          event.currentTarget.value = next === null ? '' : String(next);
          set(next);
        }}
      />
      {#if known.unit}<span class="unit muted">{known.unit}</span>{/if}
    </span>
  {/if}
{/snippet}

<div class="radeon">
  <Field
    label="Radeon preset"
    id="{uid}-profile"
    hint={info?.description ??
      'Off leaves the Radeon driver as it is. A preset changes this game’s Radeon settings while it streams and puts them back when the session ends.'}
    error={draft.radeonError || undefined}
  >
    <select
      id="{uid}-profile"
      class="select"
      bind:value={() => stored, (value) => apply(pickProfile(stored, draft.radeon, value))}
    >
      <option value="">Off</option>
      {#if unknown}
        <option value={stored}>Saved custom choice</option>
      {/if}
      {#each profiles as profile (profile.id)}
        <option value={profile.id}>{profile.label}</option>
      {/each}
    </select>
  </Field>

  {#if loading}
    <p class="hint">Reading what this PC’s Radeon GPU supports…</p>
  {:else if reason}
    <p class="note" role="status">{reason} The presets and raw settings still work.</p>
  {:else}
    <fieldset class="controls" aria-describedby="{uid}-controls-hint">
      <legend>Adjust</legend>
      <p class="hint" id="{uid}-controls-hint">
        Changing anything here makes the preset Custom. Only what is turned on is changed; everything else stays as AMD
        Software has it for this game.
      </p>
      {#if shown.has('antiLag')}
        <Toggle
          label="Anti-Lag"
          hint="Cuts the time between your input and the frame it produces."
          checked={controls.antiLag}
          onchange={(on) => change({ antiLag: on })}
        />
      {/if}
      {#if shown.has('frameCap')}
        <Field label="Frame rate cap" id="{uid}-cap">
          <div class="line">
            <select
              id="{uid}-cap"
              class="select"
              value={controls.frameCap}
              onchange={(event) => {
                const frameCap = event.currentTarget.value as FrameCap;
                change({
                  frameCap,
                  fps: frameCap === 'fps' ? (controls.fps ?? currentOf(setting('frame_rate_target.fps'))) : controls.fps,
                });
              }}
            >
              <option value="off">Off</option>
              <option value="stream">Same as the stream</option>
              <option value="fps">A number</option>
            </select>
            {#if controls.frameCap === 'fps'}
              {@render number('frame_rate_target.fps', controls.fps, 'Frame rate cap', (fps) => change({ fps }))}
            {/if}
          </div>
        </Field>
      {/if}
      {#if shown.has('chill')}
        <div class="feature">
          <Toggle
            label="Chill"
            hint="Lowers the frame rate while little moves on screen, between a minimum and a maximum."
            checked={controls.chill}
            onchange={(on) =>
              change({
                chill: on,
                chillMin: controls.chillMin ?? currentOf(setting('chill.min_fps')),
                chillMax: controls.chillMax ?? currentOf(setting('chill.max_fps')),
              })}
          />
          {#if controls.chill && (setting('chill.min_fps') || setting('chill.max_fps'))}
            <div class="line indent">
              {#if setting('chill.min_fps')}
                <span class="muted small">Min</span>
                {@render number('chill.min_fps', controls.chillMin, 'Chill minimum', (chillMin) => change({ chillMin }))}
              {/if}
              {#if setting('chill.max_fps')}
                <span class="muted small">Max</span>
                {@render number('chill.max_fps', controls.chillMax, 'Chill maximum', (chillMax) => change({ chillMax }))}
              {/if}
            </div>
          {/if}
        </div>
      {/if}
      {#if shown.has('sharpening')}
        <div class="feature">
          <Toggle
            label="Image sharpening"
            checked={controls.sharpening}
            onchange={(on) =>
              change({
                sharpening: on,
                sharpness: controls.sharpness ?? currentOf(setting('image_sharpening.sharpness')),
              })}
          />
          {#if controls.sharpening && setting('image_sharpening.sharpness')}
            <div class="line indent">
              <span class="muted small">Strength</span>
              {@render number('image_sharpening.sharpness', controls.sharpness, 'Sharpening strength', (sharpness) =>
                change({ sharpness }),
              )}
            </div>
          {/if}
        </div>
      {/if}
      {#if shown.has('anisotropic')}
        <Toggle
          label="Anisotropic filtering"
          hint="Sharper textures on surfaces seen at an angle."
          checked={controls.anisotropic}
          onchange={(on) => change({ anisotropic: on })}
        />
      {/if}
      {#if shown.has('antiAliasing')}
        <Field label="Anti-aliasing" id="{uid}-aa">
          <div class="line wrap">
            <select
              id="{uid}-aa"
              class="select"
              value={controls.antiAliasing}
              onchange={(event) => change({ antiAliasing: event.currentTarget.value as AntiAliasing })}
            >
              <option value="game">Use the game’s</option>
              <option value="enhance">Enhance the game’s</option>
              <option value="override">Override</option>
            </select>
            {#if controls.antiAliasing === 'override'}
              {#if setting('anti_aliasing.level')}
                <select
                  class="select"
                  aria-label="Anti-aliasing level"
                  value={controls.aaLevel ?? ''}
                  onchange={(event) => change({ aaLevel: event.currentTarget.value || null })}
                >
                  <option value="">Level as AMD Software has it</option>
                  {#each choices('anti_aliasing.level') as option (option.value)}
                    <option value={option.value}>{option.label}</option>
                  {/each}
                </select>
              {/if}
              {#if setting('anti_aliasing.method')}
                <select
                  class="select"
                  aria-label="Anti-aliasing method"
                  value={controls.aaMethod ?? ''}
                  onchange={(event) => change({ aaMethod: event.currentTarget.value || null })}
                >
                  <option value="">Method as AMD Software has it</option>
                  {#each choices('anti_aliasing.method') as option (option.value)}
                    <option value={option.value}>{option.label}</option>
                  {/each}
                </select>
              {/if}
            {/if}
          </div>
        </Field>
      {/if}
      {#if shown.has('boost')}
        <div class="feature">
          <Toggle
            label="Boost"
            hint="Lowers the resolution during fast motion to keep the frame rate up."
            checked={controls.boost}
            onchange={(on) =>
              change({
                boost: on,
                boostResolution: controls.boostResolution ?? currentOf(setting('boost.min_resolution')),
              })}
          />
          {#if controls.boost && setting('boost.min_resolution')}
            <div class="line indent">
              <span class="muted small">Minimum resolution</span>
              {@render number('boost.min_resolution', controls.boostResolution, 'Boost minimum resolution', (value) =>
                change({ boostResolution: value }),
              )}
            </div>
          {/if}
        </div>
      {/if}
    </fieldset>
  {/if}

  <details class="raw" bind:open={rawOpen}>
    <summary>Advanced: raw Radeon settings{raw.length ? ` (${raw.length})` : ''}</summary>
    <div class="list" aria-describedby="{uid}-list-hint">
      <p class="hint" id="{uid}-list-hint">
        Radeon setting ids the host can write, such as <code>vsync.mode</code> or <code>afmf.enabled</code>. Use true or
        false for switches. A feature's <code>.enabled</code> switch is written before its other settings.
        {#if !custom && raw.length}These come with the preset; add a setting or change a control to edit them as Custom.{/if}
      </p>
      {#each raw as row, index (row.key)}
        {@const error = custom ? rowError(row, draft.radeon) : ''}
        <div class="item">
          <input
            class="input mono"
            aria-label="Setting {index + 1}"
            aria-invalid={error ? 'true' : undefined}
            placeholder="vsync.mode"
            autocomplete="off"
            spellcheck="false"
            readonly={!custom}
            bind:value={row.id}
          />
          <input
            class="input mono value"
            aria-label="Value of setting {index + 1}"
            aria-invalid={error ? 'true' : undefined}
            placeholder="always_off"
            autocomplete="off"
            spellcheck="false"
            readonly={!custom}
            bind:value={row.value}
          />
          {#if custom}
            <button
              type="button"
              class="remove"
              title="Remove"
              aria-label="Remove setting {index + 1}"
              onclick={() => remove(row)}
            >
              <Icon name="x" size={15} />
            </button>
          {/if}
          {#if error}<p class="error">{error}</p>{/if}
        </div>
      {/each}
      <div>
        <Button size="sm" icon="plus" onclick={() => apply(addRow(stored, draft.radeon))}>Add setting</Button>
      </div>
    </div>
  </details>
</div>

<style>
  .radeon {
    display: grid;
    gap: var(--space-3);
  }
  .controls {
    display: grid;
    gap: var(--space-3);
    margin: 0;
    padding: 0;
    border: 0;
    min-width: 0;
  }
  legend {
    padding: 0;
    margin-bottom: var(--space-1);
    font-weight: var(--weight-medium);
    font-size: var(--text-sm);
  }
  .feature {
    display: grid;
    gap: var(--space-2);
  }
  .line {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    min-width: 0;
  }
  .wrap {
    flex-wrap: wrap;
  }
  .indent {
    /* Under the switch's label, past its track. */
    padding-left: calc(40px + var(--space-3));
    flex-wrap: wrap;
  }
  .line .select {
    width: auto;
    max-width: 100%;
  }
  .number {
    display: inline-flex;
    align-items: center;
    gap: var(--space-2);
  }
  .num {
    width: 100px;
  }
  .unit,
  .small {
    font-size: var(--text-sm);
  }
  .note {
    font-size: var(--text-sm);
    color: var(--muted);
    line-height: 1.45;
  }
  .raw summary {
    cursor: pointer;
    font-weight: var(--weight-medium);
    font-size: var(--text-sm);
  }
  .list {
    display: grid;
    gap: var(--space-2);
    margin-top: var(--space-2);
    min-width: 0;
  }
  .hint {
    font-size: var(--text-xs);
    color: var(--muted);
    line-height: 1.45;
  }
  .item {
    display: grid;
    grid-template-columns: minmax(0, 2fr) minmax(0, 1fr) 30px;
    gap: var(--space-2);
    align-items: center;
  }
  .error {
    grid-column: 1 / -1;
    font-size: var(--text-xs);
    color: var(--danger);
  }
  .remove {
    display: grid;
    place-items: center;
    width: 30px;
    height: 30px;
    border: 1px solid transparent;
    border-radius: var(--radius);
    background: none;
    color: var(--muted);
    cursor: pointer;
  }
  .remove:hover {
    background: var(--danger-soft);
    color: var(--danger);
  }
</style>
