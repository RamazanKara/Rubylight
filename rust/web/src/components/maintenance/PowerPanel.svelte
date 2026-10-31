<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import Button from '../Button.svelte';
  import Icon, { type IconName } from '../Icon.svelte';
  import Panel from '../Panel.svelte';
  import LoadState from '../LoadState.svelte';
  import StatusPill from '../StatusPill.svelte';
  import Toggle from '../Toggle.svelte';
  import { api, ApiError, type PowerStatus } from '../../lib/api';
  import { confirm, failed, notify } from '../../lib/feedback.svelte';
  import { ago, marked } from '../../lib/format';
  import { link } from '../../lib/router.svelte';
  import {
    adapterKind, checkName, checkPowerStatus, checkState, fastStartup, idleSleep, readiness, sleepState, systemNotes, targetState, type Tone,
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

  const CHECK_ICONS: Record<Tone, IconName> = { ok: 'check', warn: 'alert', danger: 'x', neutral: 'info' };

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
      <!-- What is wrong, then its fix, then the facts behind it. -->
      <section class="fix" aria-label="Wake-on-LAN changes">
        <p class="notice" class:warn={ready.tone === 'warn'} class:danger={ready.tone === 'danger'} role="status">{ready.text}</p>
        {#if power.fix.error}
          <p class="notice danger" role="alert">Could not read the adapters' wake settings: {power.fix.error}</p>
        {:else if changes.length}
          <p class="lead">Applying these changes lets devices wake this PC:</p>
          <ul class="changes">{#each changes as change (change)}<li>{change}</li>{/each}</ul>
        {:else if !pending && ready.tone !== 'ok'}
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
        <div class="row">
          {#if changes.length || offered}
            <Button variant="primary" busy={applying || pending} disabled={checking || pending || (!changes.length && !fastStartupOff)} onclick={apply}>Apply changes</Button>
          {/if}
          <Button icon="refresh" busy={checking} disabled={applying || pending} onclick={check}>Check again</Button>
        </div>
      </section>

      {#if power.wake.targets.length}
        <ul class="targets" aria-label="Network adapters">
          {#each power.wake.targets as target (target.mac)}
            {@const primary = target.mac === power.wake.primary}
            <!-- The adapter devices are sent already has its state in the panel's pill. -->
            {@const about = [
              ...(primary ? [] : [targetState(target).label]),
              target.connected ? 'Connected' : 'Not connected',
              ...(primary ? ['Sent to devices'] : []),
              ...(target.route ? ["This console's connection"] : []),
            ]}
            <li class="target">
              <span class="name">{adapterKind(target)} <span class="num">{target.mac}</span></span>
              <span class="about">{about.join(' · ')}</span>
              {#if target.checks.length}
                <ul class="checks">
                  {#each target.checks as item (item.id)}
                    {@const result = checkState(item)}
                    <li class={result.tone}>
                      <Icon name={CHECK_ICONS[result.tone]} size={16} />
                      <span class="check-name">{checkName(item.id)}{#if result.tone !== 'ok'}{' '}<span class="word">{result.word}</span>{/if}</span>
                    </li>
                  {/each}
                </ul>
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
          <span class="tone-{idle.tone}">{idle.title}</span>
          {#if idle.text}<span class="sub">{idle.text}</span>{/if}
          <a class="sub" href="/settings/general?setting=sleep_after_idle_minutes" use:link>Change sleep after idle</a>
        </dd>
        <dt>Last sleep</dt>
        <dd>
          {#if !power.last_sleep}<span class="muted">Not since Rubylight started</span>
          {:else if power.last_sleep.error}<span class="tone-danger">Failed {ago(power.last_sleep.at)}: {power.last_sleep.error}</span>
          {:else}{ago(power.last_sleep.at)}{/if}
        </dd>
      </dl>
      {#if notes.length}<ul class="notes">{#each notes as note (note)}<li>{note}</li>{/each}</ul>{/if}
    </div>
  {:else}<LoadState error={loadError} what="the wake status" slow="The first check reads every network adapter and can take half a minute." onretry={() => load(true)} />{/if}
</Panel>

<style>
  .fix {
    display: grid;
    gap: var(--space-3);
  }
  .targets {
    display: grid;
    gap: var(--space-3);
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .target {
    display: grid;
    gap: var(--space-1);
    padding: var(--space-3) var(--space-4);
    border: 1px solid var(--line);
    border-radius: var(--radius);
    min-width: 0;
  }
  .name {
    font-weight: var(--weight-semibold);
    overflow-wrap: anywhere;
  }
  .about {
    color: var(--muted);
    font-size: var(--text-sm);
  }
  .checks {
    display: grid;
    gap: var(--space-1);
    margin: var(--space-2) 0 0;
    padding: 0;
    list-style: none;
    font-size: var(--text-sm);
  }
  /* The name wraps beside its icon; the word keeps its width and follows the name. */
  .checks li {
    display: grid;
    grid-template-columns: 16px minmax(0, 1fr);
    align-items: start;
    gap: var(--space-2);
  }
  .checks :global(svg) {
    margin-top: 2px;
  }
  .check-name {
    color: var(--ink-2);
  }
  .word {
    margin-left: var(--space-1);
    white-space: nowrap;
    font-weight: var(--weight-semibold);
  }
  .ok :global(svg) { color: var(--ok); }
  .warn :global(svg), .warn .word, .tone-warn { color: var(--warn); }
  .danger :global(svg), .danger .word, .tone-danger { color: var(--danger); }
  .neutral :global(svg) { color: var(--muted); }
  .sub {
    display: block;
    width: fit-content;
    margin-top: 2px;
    color: var(--muted);
  }
  a.sub {
    color: var(--ink-2);
  }
  .lead {
    margin: 0;
  }
  .changes,
  .notice ul {
    margin: 0;
    padding-left: var(--space-5);
  }
  .notes {
    display: grid;
    gap: var(--space-1);
    margin: 0;
    padding-left: var(--space-5);
    color: var(--muted);
    font-size: var(--text-sm);
  }
</style>
