<script lang="ts">
  import Button from '../Button.svelte';
  import Field from '../Field.svelte';
  import Icon from '../Icon.svelte';
  import type { AppDraft } from './draft.svelte';
  import { PROFILE_KEY, profileOf, profiles, rowError, settingRow } from './radeon';

  let { draft }: { draft: AppDraft } = $props();

  const uid = $props.id();

  const stored = $derived(draft.text(PROFILE_KEY as 'radeon-profile'));
  const chosen = $derived(profileOf(stored));
  const info = $derived(profiles.find((profile) => profile.id === chosen));
  // A profile this console does not know is kept as written and the host says what it thinks.
  const unknown = $derived(stored !== '' && !info);
</script>

<div class="radeon">
  <Field
    label="Radeon profile"
    id="{uid}-profile"
    hint={info?.description ?? 'Changes Radeon driver settings while this app runs and puts them back when the session ends.'}
    error={draft.radeonError && chosen !== 'custom' ? draft.radeonError : undefined}
  >
    <select
      id="{uid}-profile"
      class="select"
      bind:value={() => stored, (value) => draft.setText(PROFILE_KEY as 'radeon-profile', value)}
    >
      <option value="">None</option>
      {#if unknown}
        <option value={stored}>Saved custom choice</option>
      {/if}
      {#each profiles as profile (profile.id)}
        <option value={profile.id}>{profile.label}</option>
      {/each}
    </select>
  </Field>

  {#if chosen === 'custom'}
    <fieldset class="list" aria-describedby="{uid}-list-hint">
      <legend>Settings</legend>
      <p class="hint" id="{uid}-list-hint">
        Radeon setting ids the host can write, such as <code>frame_rate_target.fps</code>. Use true or false for switches. A
        feature's <code>.enabled</code> switch is written before its other settings.
      </p>
      {#each draft.radeon as row, index (row.key)}
        {@const error = rowError(row, draft.radeon)}
        <div class="item">
          <input
            class="input mono"
            aria-label="Setting {index + 1}"
            aria-invalid={error ? 'true' : undefined}
            placeholder="frame_rate_target.fps"
            autocomplete="off"
            spellcheck="false"
            bind:value={row.id}
          />
          <input
            class="input mono value"
            aria-label="Value of setting {index + 1}"
            aria-invalid={error ? 'true' : undefined}
            placeholder="60"
            autocomplete="off"
            spellcheck="false"
            bind:value={row.value}
          />
          <button
            type="button"
            class="remove"
            title="Remove"
            aria-label="Remove setting {index + 1}"
            onclick={() => draft.radeon.splice(index, 1)}
          >
            <Icon name="x" size={15} />
          </button>
          {#if error}<p class="error">{error}</p>{/if}
        </div>
      {/each}
      {#if draft.radeonError}<p class="error">{draft.radeonError}</p>{/if}
      <div>
        <Button size="sm" icon="plus" onclick={() => draft.radeon.push(settingRow())}>Add setting</Button>
      </div>
    </fieldset>
  {/if}
</div>

<style>
  .radeon {
    display: grid;
    gap: var(--space-3);
  }
  .list {
    display: grid;
    gap: var(--space-2);
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
  .hint {
    font-size: var(--text-xs);
    color: var(--muted);
    line-height: 1.45;
    margin-top: -4px;
  }
  .item {
    display: grid;
    grid-template-columns: minmax(0, 2fr) minmax(0, 1fr) auto;
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
