<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import Badge from '../Badge.svelte';
  import Button from '../Button.svelte';
  import Panel from '../Panel.svelte';
  import LoadState from '../LoadState.svelte';
  import StatusPill from '../StatusPill.svelte';
  import Toggle from '../Toggle.svelte';
  import { api, ApiError, type PowerStatus } from '../../lib/api';
  import { confirm, failed, notify } from '../../lib/feedback.svelte';
  import { ago, marked } from '../../lib/format';
  import { link } from '../../lib/router.svelte';
  import {
    adapterKind, checkName, checkPowerStatus, checkState, fastStartup, idleSleep, readiness, sleepState, systemNotes, targetState,
  } from '../../lib/power';
  import { reason } from './util';

  let power = $state<PowerStatus | null>(null);
  let loadError = $state('');
  let checking = $state(false);
  let applying = $state(false);
  let fastStartupOff = $state(false);
  /** `fix.last.at` before this console asked for a fix; a new one is its result. */
  let awaiting = $state<{ previous: number | null } | null>(null);
  let timer: ReturnType<typeof setTimeout>;
  let alive = true;
  onDestroy(() => { alive = false; clearTimeout(timer); });

  const ready = $derived(power ? readiness(power) : null);
  const idle = $derived(power ? idleSleep(power) : null);
  const notes = $derived(power ? systemNotes(power) : []);
  const changes = $derived(power?.fix.changes ?? []);
  const offered = $derived(power?.fix.fast_startup_off_offered ?? false);
  const pending = $derived(power?.fix.running === true || awaiting !== null);
  const last = $derived(power?.fix.last ?? null);

  async function load(refresh = false) {
    clearTimeout(timer);
    if (!alive) return;
    try {
      power = checkPowerStatus(await api.power.status(refresh));
      loadError = '';
      // The host marks the fix running before it answers 202, so a status that
      // is not running has its result, or lost it when the host restarted.
      const done = power.fix.last;
      if (awaiting && !power.fix.running) {
        const lost = !done || done.at === awaiting.previous;
        awaiting = null;
        if (lost) notify('The host restarted before it reported the Wake-on-LAN changes. Check again to see the settings now.', 'danger');
        else if (done.errors.length) notify('Some Wake-on-LAN changes failed. See Wake-on-LAN and sleep.', 'danger');
        else notify('Wake-on-LAN changes applied.', 'ok');
      }
    } catch (error) {
      loadError = reason(error);
    } finally {
      if (alive && !document.hidden) {
        // A poll, not someone at the console (sleep after idle). The countdown moves every minute.
        timer = setTimeout(() => marked(load), pending ? 2000 : 30000);
      }
    }
  }
  async function check() {
    if (checking) return;
    checking = true;
    try { await load(true); }
    finally { checking = false; }
  }
  async function apply() {
    if (!power || applying || pending) return;
    const turnOff = offered && fastStartupOff;
    const count = changes.length + (turnOff ? 1 : 0);
    if (!count) return;
    if (!(await confirm({
      title: 'Change the wake settings?',
      message: `Rubylight makes ${count === 1 ? 'this change' : `these ${count} changes`} to Windows. Adapters whose settings change restart, so the network drops for a few seconds and this page may lose the host briefly. Changes are refused while a stream is active or starting.`,
      confirm: 'Apply changes',
    }))) return;
    applying = true;
    const previous = power.fix.last?.at ?? null;
    let refresh = false;
    try {
      const result = await api.power.fixWake(changes, turnOff);
      if (result.changes.length) awaiting = { previous };
      else notify('Nothing to change: the wake settings are already on.', 'ok');
    } catch (error) {
      failed('Wake-on-LAN changes not applied', error);
      // The adapters changed since the check: show what a fix would do now.
      refresh = error instanceof ApiError && error.status === 409;
    } finally {
      applying = false;
    }
    await load(refresh);
  }
  onMount(() => {
    const visibility = () => {
      clearTimeout(timer);
      if (!document.hidden) void load();
    };
    document.addEventListener('visibilitychange', visibility);
    void load();
    return () => document.removeEventListener('visibilitychange', visibility);
  });
</script>

<Panel title="Wake-on-LAN and sleep" level={3} description="Wake this PC from a device, and let it sleep when nobody uses it.">
  {#snippet actions()}
    {#if ready}<StatusPill status={ready.status} label={ready.label} />{/if}
  {/snippet}
  {#if power && ready && idle}
    <div class="stack">
      {#if loadError}<LoadState error={loadError} stale what="the wake status" onretry={() => load(true)} />{/if}
      <p class="notice" class:warn={ready.tone === 'warn'} class:danger={ready.tone === 'danger'} role="status">{ready.text}</p>
      {#if power.wake.targets.length}
        <ul class="targets" aria-label="Network adapters">
          {#each power.wake.targets as target (target.mac)}
            {@const state = targetState(target)}
            <li class="target">
              <div class="head">
                <span class="name">{adapterKind(target)} <span class="num">{target.mac}</span></span>
                <StatusPill status={state.status} label={state.label} />
              </div>
              <div class="tags">
                <Badge tone={target.connected ? 'ok' : 'neutral'}>{target.connected ? 'Connected' : 'Not connected'}</Badge>
                {#if target.mac === power.wake.primary}<Badge tone="accent">Sent to devices</Badge>{/if}
                {#if target.route}<Badge>This console's connection</Badge>{/if}
              </div>
              {#if target.checks.length}
                <dl class="facts checks">
                  {#each target.checks as item (item.id)}
                    {@const result = checkState(item)}
                    <dt>{checkName(item.id)}</dt><dd><Badge tone={result.tone}>{result.word}</Badge></dd>
                  {/each}
                </dl>
              {/if}
            </li>
          {/each}
        </ul>
      {/if}
      <dl class="facts">
        <dt>Sleep</dt><dd>{sleepState(power)}</dd>
        <dt>Fast Startup</dt><dd>{fastStartup(power)}</dd>
        <dt>Sleep after idle</dt>
        <dd>
          <Badge tone={idle.tone}>{idle.title}</Badge>
          {#if idle.text}<span class="muted"> {idle.text}</span>{/if}
        </dd>
        <dt>Last sleep</dt>
        <dd>
          {#if !power.last_sleep}<span class="muted">Not since Rubylight started</span>
          {:else if power.last_sleep.error}<span class="error">Failed {ago(power.last_sleep.at)}: {power.last_sleep.error}</span>
          {:else}{ago(power.last_sleep.at)}{/if}
        </dd>
      </dl>
      {#each notes as note (note)}<p class="notice">{note}</p>{/each}

      <section class="fix" aria-label="Wake-on-LAN changes">
        {#if power.fix.error}
          <p class="notice danger" role="alert">Could not read the adapters' wake settings: {power.fix.error}</p>
        {:else if changes.length}
          <p class="lead">Applying these changes lets devices wake this PC:</p>
          <ul class="changes">{#each changes as change (change)}<li>{change}</li>{/each}</ul>
        {:else if !pending}
          <p class="muted lead">The wake settings need no changes.</p>
        {/if}
        {#each power.fix.skipped ?? [] as skipped (skipped)}<p class="notice warn">Not changed: {skipped}</p>{/each}
        {#if offered}
          <Toggle label="Also turn off Fast Startup" hint="Lets adapters wake the PC after Shut down. Windows then starts a little slower." bind:checked={fastStartupOff} disabled={pending} />
        {/if}
        {#if pending}
          <p class="notice" role="status">Applying the changes. The network may drop for a few seconds…</p>
        {:else if last}
          {#if last.errors.length}
            <div class="notice danger" role="alert">
              <span>The last change {ago(last.at)} failed:</span>
              <ul>{#each last.errors as error (error)}<li>{error}</li>{/each}</ul>
            </div>
          {:else}
            <p class="notice" role="status">
              Applied {last.changes?.length ?? 0} {last.changes?.length === 1 ? 'change' : 'changes'} {ago(last.at)}.
              {#if last.restarted?.length}Restarted: {last.restarted.join(', ')}.{/if}
              {#if last.after_reboot?.length}Takes effect after the PC restarts: {last.after_reboot.join(', ')}.{/if}
            </p>
          {/if}
        {/if}
      </section>

      <div class="row">
        {#if changes.length || offered}
          <Button variant="primary" busy={applying || pending} disabled={checking || pending || (!changes.length && !fastStartupOff)} onclick={apply}>Apply changes</Button>
        {/if}
        <Button icon="refresh" busy={checking} disabled={applying || pending} onclick={check}>Check again</Button>
        <a class="small" href="/settings/general" use:link>Change sleep after idle</a>
      </div>
    </div>
  {:else}<LoadState error={loadError} what="the wake status" onretry={() => load(true)} />{/if}
</Panel>

<style>
  .targets {
    display: grid;
    gap: var(--space-3);
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .target {
    display: grid;
    gap: var(--space-2);
    padding: var(--space-3);
    border: 1px solid var(--line);
    border-radius: var(--radius);
    min-width: 0;
  }
  .head {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-2);
  }
  .name {
    font-weight: var(--weight-medium);
    overflow-wrap: anywhere;
  }
  .tags {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-2);
  }
  .checks {
    font-size: var(--text-sm);
  }
  .fix {
    display: grid;
    gap: var(--space-2);
  }
  .lead {
    margin: 0;
  }
  .changes,
  .notice ul {
    margin: 0;
    padding-left: var(--space-5);
  }
  .error {
    color: var(--danger);
  }
  .small {
    font-size: var(--text-sm);
  }
</style>
