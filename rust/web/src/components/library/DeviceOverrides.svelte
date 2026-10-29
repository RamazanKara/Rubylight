<script lang="ts">
  import { onMount } from 'svelte';
  import Button from '../Button.svelte';
  import Field from '../Field.svelte';
  import GameFiles from './GameFiles.svelte';
  import { api, type Client } from '../../lib/api';
  import { presets } from '../../lib/stream-presets';
  import { profileOf, profiles } from './radeon';
  import { CONFIG_KEY, FRAME_LIMIT, STREAM_PRESET, frameLimitError, type AppDraft } from './draft.svelte';

  let { draft }: { draft: AppDraft } = $props();

  const uid = $props.id();
  /** The Radeon choice that leaves the app's profile in place. */
  const SAME = 'same-as-app';

  let clients = $state<Client[]>([]);
  let loading = $state(true);
  let readError = $state('');
  let adding = $state('');
  onMount(() => {
    api.clients
      .list()
      .then((list) => (clients = list.clients))
      .catch((error: unknown) => (readError = error instanceof Error ? error.message : String(error)))
      .finally(() => (loading = false));
  });

  const listed = $derived(draft.devices());
  const available = $derived(clients.filter((client) => !listed.includes(client.uuid)));
  const nameOf = (device: string) => clients.find((client) => client.uuid === device)?.name;

  const text = (value: unknown) => (typeof value === 'string' || typeof value === 'number' ? String(value) : '');
  /** A number when the text reads back the same, else the text, so typing "59." is not undone. */
  function rate(typed: string): string | number | undefined {
    const value = typed.trim();
    if (!value) return undefined;
    return String(Number(value)) === value ? Number(value) : value;
  }
  /** The general settings of a device's block that this page does not show. */
  function others(device: string): string[] {
    const block = draft.app['device-overrides'] as Record<string, Record<string, unknown>> | undefined;
    const config = block?.[device]?.[CONFIG_KEY];
    if (!config || typeof config !== 'object') return [];
    return Object.keys(config).filter((key) => key !== STREAM_PRESET && key !== FRAME_LIMIT);
  }

  function add() {
    if (!adding) return;
    draft.addDevice(adding);
    adding = '';
  }
</script>

<div class="devices">
  <p class="hint">
    Settings for this game on one device, such as 60 fps on the TV and 120 on a handheld. They win over the game’s own
    settings and the device’s.
  </p>

  {#each listed as device (device)}
    {@const name = nameOf(device)}
    {@const preset = text(draft.deviceSetting(device, STREAM_PRESET))}
    {@const radeon = draft.deviceRadeon(device)}
    {@const limit = draft.deviceSetting(device, FRAME_LIMIT)}
    {@const kept = others(device)}
    <section class="device" aria-label={name ?? 'A device that is not paired'}>
      <header>
        <h3>{name ?? (loading ? 'Device' : 'A device that is not paired')}</h3>
        <Button size="sm" icon="trash" onclick={() => draft.removeDevice(device)}>Remove</Button>
      </header>
      {#if !name && !loading}
        <p class="note">Kept as saved. The host ignores these settings unless this device is paired again.</p>
      {/if}
      <div class="form-grid">
        <Field label="Stream preset" id="{uid}-{device}-preset">
          <select
            id="{uid}-{device}-preset"
            class="select"
            value={preset}
            onchange={(event) => draft.setDeviceSetting(device, STREAM_PRESET, event.currentTarget.value || undefined)}
          >
            <option value="">Same as the game</option>
            {#if preset && !presets.some((known) => known.id === preset)}
              <option value={preset}>{preset} (not a preset)</option>
            {/if}
            {#each presets as known (known.id)}
              <option value={known.id}>{known.label}</option>
            {/each}
          </select>
        </Field>
        <Field
          label="Frame limit"
          id="{uid}-{device}-limit"
          hint="Games are capped at this rate on this device; 0 is the stream’s frame rate. Empty uses the game’s or the host’s."
          error={frameLimitError(limit) || undefined}
        >
          <span class="number">
            <input
              id="{uid}-{device}-limit"
              class="input num"
              inputmode="decimal"
              autocomplete="off"
              placeholder="Same"
              value={text(limit)}
              oninput={(event) => draft.setDeviceSetting(device, FRAME_LIMIT, rate(event.currentTarget.value))}
            />
            <span class="unit muted">fps</span>
          </span>
        </Field>
        <Field label="Radeon preset" id="{uid}-{device}-radeon">
          <select
            id="{uid}-{device}-radeon"
            class="select"
            value={radeon ?? SAME}
            onchange={(event) => {
              const value = event.currentTarget.value;
              draft.setDeviceRadeon(device, value === SAME ? null : value);
            }}
          >
            <option value={SAME}>Same as the game</option>
            <option value="">Off</option>
            {#if radeon && !profiles.some((known) => known.id !== 'custom' && known.id === profileOf(radeon))}
              <option value={radeon}>Saved custom choice</option>
            {/if}
            {#each profiles.filter((known) => known.id !== 'custom') as known (known.id)}
              <option value={known.id}>{known.label}</option>
            {/each}
          </select>
        </Field>
      </div>
      {#if kept.length}
        <p class="note">Also kept for this device: {kept.join(', ')}.</p>
      {/if}
      {#if draft.deviceFiles[device]}
        <div class="files">
          <h4>Managed game files on this device</h4>
          <p class="note">A key listed here replaces the game’s entry for the same file, section and key.</p>
          <GameFiles id="{uid}-{device}-files" bind:rows={draft.deviceFiles[device]} />
        </div>
      {/if}
    </section>
  {/each}

  {#if loading}
    <p class="hint">Reading the paired devices…</p>
  {:else if readError}
    <p class="note" role="status">The paired devices could not be read: {readError}</p>
  {:else if available.length}
    <div class="add">
      <select class="select" aria-label="Device to add" bind:value={adding}>
        <option value="">Choose a device…</option>
        {#each available as client (client.uuid)}
          <option value={client.uuid}>{client.name}</option>
        {/each}
      </select>
      <Button size="sm" icon="plus" disabled={!adding} onclick={add}>Add device</Button>
    </div>
  {:else if !clients.length}
    <p class="hint">Pair a device first; its settings for this game can then be set here.</p>
  {/if}
</div>

<style>
  .devices {
    display: grid;
    gap: var(--space-4);
    min-width: 0;
  }
  .device {
    display: grid;
    gap: var(--space-3);
    padding: var(--space-4);
    border: 1px solid var(--line);
    border-radius: var(--radius-lg);
    min-width: 0;
  }
  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-2);
  }
  h3 {
    font-size: var(--text-sm);
    font-weight: var(--weight-medium);
  }
  h4 {
    font-size: var(--text-sm);
    font-weight: var(--weight-medium);
  }
  .files {
    display: grid;
    gap: var(--space-2);
  }
  .number {
    display: inline-flex;
    align-items: center;
    gap: var(--space-2);
  }
  .num {
    width: 100px;
  }
  .unit {
    font-size: var(--text-sm);
  }
  .add {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    flex-wrap: wrap;
  }
  .add .select {
    width: auto;
    max-width: 100%;
  }
  .hint,
  .note {
    font-size: var(--text-xs);
    color: var(--muted);
    line-height: 1.45;
  }
</style>
