<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import Button from '../Button.svelte';
  import StatusPill from '../StatusPill.svelte';
  import { api, ApiError, type PowerStatus } from '../../lib/api';
  import { failed, notify } from '../../lib/feedback.svelte';
  import { link } from '../../lib/router.svelte';
  import { checkPowerStatus, readiness } from '../../lib/power';
  import { reason, sleep } from '../maintenance/util';

  // Wake-on-LAN on the Settings home: one row and one Turn on, which makes the
  // adapter changes the detail panel (All settings › This PC › Sleep and wake)
  // lists. It reads the status once; it polls only while its own change runs.
  let { details }: { details: string } = $props();

  let power = $state<PowerStatus | null>(null);
  let loadError = $state('');
  let turning = $state(false);
  let alive = true;
  onDestroy(() => (alive = false));

  const ready = $derived(power ? readiness(power) : null);
  const changes = $derived(power?.fix.changes ?? []);
  /** Something to turn on that Rubylight can change itself. */
  const offer = $derived(!!power && !power.fix.error && changes.length > 0);
  const running = $derived(turning || power?.fix.running === true);

  const text = $derived.by(() => {
    if (running) return 'Turning on. The network drops for a few seconds.';
    if (!power) return loadError ? `Could not check: ${loadError}` : 'Checking the network adapters…';
    if (power.fix.error) return "The adapters' wake settings could not be read.";
    if (offer) return 'Lets your devices start this PC from sleep. Turning it on changes the wake settings of the network adapter; the network may drop for a few seconds.';
    return ready?.text ?? '';
  });

  async function load(refresh = false) {
    try {
      power = checkPowerStatus(await api.power.status(refresh));
      loadError = '';
    } catch (error) {
      loadError = reason(error);
    }
  }

  async function turnOn() {
    if (!power || running || !offer) return;
    const previous = power.fix.last?.at ?? null;
    turning = true;
    try {
      const result = await api.power.fixWake(changes, false);
      if (result.changes.length) await finished(previous);
      else notify('Wake is already on.', 'ok');
    } catch (error) {
      failed('Wake not turned on', error);
      // The adapters changed since the check: show what a change would do now.
      if (error instanceof ApiError && error.status === 409) await load(true);
    } finally {
      turning = false;
    }
    if (alive) await load();
  }

  /** The host makes the change about a second after it answers: wait for its result, a minute at most. */
  async function finished(previous: number | null) {
    const started = Date.now();
    while (alive && Date.now() - started < 60_000) {
      await sleep(2000);
      try {
        const status = checkPowerStatus(await api.power.status());
        if (status.fix.running) continue;
        power = status;
        const done = status.fix.last;
        if (!done || done.at === previous) notify('The PC restarted before it reported the wake changes. Open the wake details to check them.', 'danger');
        else if (done.errors.length) notify('Some wake changes failed. The wake details list them.', 'danger');
        else notify('Wake is on.', 'ok');
        return;
      } catch {
        // The network drops while an adapter restarts: ask again.
      }
    }
  }

  onMount(() => void load());
</script>

<div class="setting wake" data-wake>
  <div class="layout">
    <div class="text">
      <span class="label">Wake this PC from a device</span>
      <p class="description">
        {text}
        <a href={details} use:link>Wake details</a>
      </p>
    </div>
    <div class="control">
      {#if offer || running}
        <Button variant="primary" busy={running} onclick={turnOn}>Turn on<span class="visually-hidden"> wake from a device</span></Button>
      {:else if !power && loadError}
        <Button icon="refresh" onclick={() => load(true)}>Retry<span class="visually-hidden"> the wake status</span></Button>
      {:else if ready}
        <StatusPill status={ready.tone === 'ok' ? 'ready' : ready.status} label={ready.tone === 'ok' ? 'On' : ready.label} />
      {/if}
    </div>
  </div>
</div>

<style>
  /* The same row as a setting's (SettingRow.svelte): words left, the control right. */
  .setting {
    container: wake / inline-size;
    min-width: 0;
  }
  .layout {
    display: grid;
    grid-template-columns: minmax(0, 1fr) 320px;
    align-items: start;
    gap: 6px var(--space-6);
    padding: 14px var(--space-5);
  }
  .text {
    display: grid;
    gap: 3px;
    min-width: 0;
  }
  .label {
    min-height: 22px;
    font-weight: var(--weight-medium);
    font-size: var(--text);
    line-height: 1.35;
  }
  .description {
    max-width: 68ch;
    font-size: var(--text-sm);
    line-height: 1.45;
    color: var(--muted);
  }
  .description a {
    margin-left: var(--space-1);
    color: var(--ink-2);
    white-space: nowrap;
  }
  .description a:hover {
    color: var(--ink);
  }
  .control {
    display: flex;
    justify-content: flex-end;
    align-items: center;
    min-width: 0;
  }
  @container wake (max-width: 620px) {
    .layout {
      grid-template-columns: minmax(0, 1fr) auto;
      column-gap: var(--space-4);
      padding: var(--space-3) var(--space-4);
    }
  }
</style>
