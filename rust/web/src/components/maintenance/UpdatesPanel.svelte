<script lang="ts">
  import { onDestroy, onMount, type Snippet } from 'svelte';
  import Badge from '../Badge.svelte';
  import Button from '../Button.svelte';
  import Icon from '../Icon.svelte';
  import Panel from '../Panel.svelte';
  import LoadState from '../LoadState.svelte';
  import { api, type UpdatesState } from '../../lib/api';
  import { confirm, failed } from '../../lib/feedback.svelte';
  import { ago, marked, when } from '../../lib/format';
  import { link } from '../../lib/router.svelte';
  import { refreshMetadata, session } from '../../lib/session.svelte';
  import { localTime, reason } from './util';

  /** `settings`: update settings shown above the status (the Settings home). */
  let { title = 'Updates', settings }: { title?: string; settings?: Snippet } = $props();

  let updates = $state<UpdatesState | null>(null);
  let loadError = $state('');
  type Action = 'check' | 'install' | 'installNow' | 'cancel';
  /** The request in flight, so only its own button spins. */
  let acting = $state<Action | null>(null);
  let reconnecting = $state(false);
  let timer: ReturnType<typeof setTimeout>;
  let alive = true;
  onDestroy(() => { alive = false; clearTimeout(timer); });
  const installed = $derived(session.metadata?.version ?? '');
  const latest = $derived(updates?.releases.find((release) => release.tag_name === updates?.latest_version) ?? updates?.releases[0] ?? null);
  const notes = $derived(latest?.body?.trim() ?? '');
  const available = $derived(updates?.update_available ?? false);
  const queued = $derived(['waiting', 'downloading', 'ready', 'installing'].includes(updates?.phase ?? ''));
  /** The last install's outcome, unless the failure notice above already says it. */
  const lastInstall = $derived(queued || (updates?.phase === 'failed' && updates.last_install?.error === updates.error) ? null : updates?.last_install ?? null);
  const progress = $derived(Math.min(100, Math.floor(100 * (updates?.downloaded_bytes ?? 0) / (updates?.download_size || 1))));

  async function load() {
    clearTimeout(timer);
    if (!alive || document.hidden) return;
    try {
      const previous = updates?.phase;
      updates = await api.updates.state();
      loadError = '';
      reconnecting = false;
      // Back from an install: the sidebar should show the new version. A failure here is not an update error.
      if (previous === 'installing' && updates.phase !== 'installing') refreshMetadata().catch(() => {});
    } catch (error) {
      if (updates?.phase === 'installing') reconnecting = true;
      else loadError = reason(error);
    } finally {
      if (alive && !document.hidden) {
        const active = updates?.checking || queued || updates?.last_install?.phase === 'installing';
        // A poll, not someone at the console (sleep after idle).
        timer = setTimeout(() => marked(load), active ? 2000 : 30000);
      }
    }
  }
  async function act(action: Action) {
    if (acting) return;
    if (action === 'installNow' && !(await confirm({
      title: 'Install now and end streams?',
      message: 'Active streams will disconnect and games started from Moonlight will close. Save your game progress before continuing. Rubylight will restart after installation.',
      confirm: 'Install now',
      danger: true,
    }))) return;
    acting = action;
    try { await api.updates[action](); await load(); }
    catch (error) { failed('Update request failed', error); }
    finally { acting = null; }
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

<Panel {title} level={3}>
  {#snippet actions()}
    {#if available}<Badge tone="accent">Update available</Badge>
    {:else if latest && !updates?.check_failed}<Badge tone="ok">Up to date</Badge>{/if}
  {/snippet}
  {#if settings}<div class="settings">{@render settings()}</div>{/if}
  {#if updates}
    <div class="stack">
      <!-- The versions and Check now stay in view; schedule, notes and links fold under More. -->
      <dl class="facts">
        <dt>Installed</dt><dd class="num">{installed || '…'}</dd>
        <dt>Latest release</dt>
        <dd>
          {#if latest}<span class="num">{latest.tag_name}</span>
            {#if latest.prerelease}<Badge>Pre-release</Badge>{/if}
            {#if latest.published_at}<span class="muted"> · {localTime(latest.published_at)}</span>{/if}
          {:else}<span class="muted">{updates.checked_at ? 'No releases found for your update settings' : 'Not checked yet'}</span>{/if}
        </dd>
      </dl>
      {#if updates.check_failed}<p class="notice warn" role="alert">The last check failed. {updates.check_error || 'Check that this PC can reach GitHub.'}{updates.next_check_at != null ? ' Rubylight tries again automatically.' : ''}</p>{/if}
      {#if loadError}<p class="notice warn" role="alert">Could not refresh update status: {loadError}</p>{/if}
      {#if (updates.phase === 'waiting' || updates.phase === 'ready') && updates.now}
        <p class="notice" role="status">Installing {updates.queued_version} now…</p>
      {:else if updates.phase === 'waiting' || updates.phase === 'ready'}
        <p class="notice" role="status">{updates.queued_version} is queued. Waiting for the host to be idle. Disconnect devices and close the running app to continue, or choose Install now.</p>
      {:else if updates.phase === 'downloading'}
        <p class="notice" role="status">Downloading {updates.queued_version}: {progress}%</p>
        <progress max="100" value={progress} aria-label="Update download">{progress}%</progress>
      {:else if updates.phase === 'installing'}
        <p class="notice" role="status">{reconnecting ? 'Rubylight is restarting. Reconnecting…' : 'Installing the update. Rubylight will restart shortly…'}</p>
      {:else if updates.phase === 'failed'}
        <p class="notice warn" role="alert">{updates.error}</p>
      {/if}
      {#if lastInstall}
        <p class:warn={lastInstall.phase !== 'installed'} class="notice" role="status">
          {#if lastInstall.phase === 'installed'}Installed {lastInstall.version} successfully.
          {:else if lastInstall.error}{lastInstall.error}
          {:else}The previous update did not report completion. Check the setup log before retrying.{/if}
        </p>
      {/if}
      <div class="row">
        <Button icon="refresh" busy={acting === 'check' || updates.checking} disabled={queued || acting !== null} onclick={() => act('check')}>Check now</Button>
        {#if queued && updates.phase !== 'installing'}
          {#if !updates.now}<Button variant="danger" busy={acting === 'installNow'} disabled={acting !== null} onclick={() => act('installNow')}>Install now</Button>{/if}
          <Button busy={acting === 'cancel'} disabled={acting !== null} onclick={() => act('cancel')}>Cancel update</Button>
        {:else if available && updates.install_supported && !queued}
          <Button variant="primary" busy={acting === 'install'} disabled={acting !== null} onclick={() => act('install')}>Install when idle</Button>
          <Button variant="danger" busy={acting === 'installNow'} disabled={acting !== null} onclick={() => act('installNow')}>Install now</Button>
        {/if}
      </div>
      <details class="more">
        <summary>More<span class="visually-hidden"> about updates</span></summary>
        <div class="links">
        <dl class="facts">
          <dt>Last checked</dt><dd>{updates.checked_at ? `${ago(updates.checked_at)}${updates.check_failed ? ' (failed)' : ''}` : 'Never'}</dd>
          {#if updates.next_check_at != null}
            <dt>Next check</dt><dd>{updates.deferred ? 'After the stream' : updates.next_check_at * 1000 <= Date.now() ? 'Shortly' : when(updates.next_check_at)}</dd>
          {/if}
          <dt>Automatic installation</dt><dd>{!updates.install_supported ? 'Unavailable — requires the Windows service' : updates.update_check_interval <= 0 ? 'Off — automatic update checks are disabled' : updates.auto_update ? 'On — installs when idle' : 'Off — notify me first'}</dd>
        </dl>
        {#if !updates.install_supported}<p class="notice">In-app installation requires the installed Windows service. Use the release page for portable builds.</p>{/if}
        <details>
          <summary>How installing works</summary>
          <p><strong>Install when idle</strong> waits for streams, pending connections and host apps to stop, then installs after one minute of idle time. <strong>Install now</strong> installs right away: the stream ends, games started from Moonlight close, and you can connect again once Rubylight has restarted. Settings and paired devices stay in place.</p>
        </details>
        {#if latest && notes}<details><summary>Release notes for {latest.name || latest.tag_name}</summary><pre class="notes">{notes}</pre></details>{/if}
        <div class="row small">
          <a href="/settings/general?setting=update_check_interval" use:link>Change update settings</a>
          {#if latest}<a class="release" href={latest.html_url} target="_blank" rel="noreferrer">Open the release page<Icon name="external" size={14} /></a>{/if}
        </div>
        </div>
      </details>
    </div>
  {:else}<LoadState error={loadError} what="the update status" onretry={load} />{/if}
</Panel>

<style>
  /* Setting rows reach the panel's edges, above the status. */
  .settings {
    margin: calc(-1 * var(--space-5)) calc(-1 * var(--space-5)) var(--space-4);
    border-bottom: 1px solid var(--line);
  }
  @media (max-width: 520px) {
    .settings {
      margin: calc(-1 * var(--space-4)) calc(-1 * var(--space-4)) var(--space-4);
    }
  }
  .links {
    display: grid;
    gap: var(--space-3);
    margin-top: var(--space-2);
  }
  /* The same More as a setting row's. */
  .more > summary {
    display: inline-flex;
    align-items: center;
    gap: var(--space-1);
    width: fit-content;
    padding: 2px 0;
    border: 0;
    color: var(--ink-2);
    font-size: var(--text-xs);
    font-weight: var(--weight-medium);
    cursor: pointer;
    list-style: none;
  }
  .more > summary::-webkit-details-marker {
    display: none;
  }
  .more > summary::after {
    content: '';
    width: 5px;
    height: 5px;
    margin-left: 2px;
    border-right: 1.5px solid currentColor;
    border-bottom: 1.5px solid currentColor;
    transform: translateY(-2px) rotate(45deg);
    transition: transform var(--duration-fast);
  }
  .more[open] > summary::after {
    transform: translateY(1px) rotate(225deg);
  }
  .more > summary:hover {
    color: var(--ink);
    text-decoration: underline;
    text-underline-offset: 2px;
  }
  @media (max-width: 520px) {
    .more > summary {
      min-height: var(--control-height);
    }
  }
  .links details {
    border: 1px solid var(--line);
    border-radius: var(--radius);
    font-size: var(--text-sm);
  }
  .links summary {
    padding: var(--space-2) var(--space-3);
    cursor: pointer;
    font-weight: var(--weight-medium);
  }
  .links details p {
    padding: 0 var(--space-3) var(--space-3);
    color: var(--ink-2);
  }
  .notes {
    margin: 0;
    padding: var(--space-3);
    border-top: 1px solid var(--line);
    background: var(--sunken);
    max-height: 320px;
    overflow: auto;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    font-size: var(--text-xs);
    line-height: 1.55;
  }
  .small {
    gap: var(--space-2) var(--space-4);
    font-size: var(--text-sm);
  }
  .release {
    display: inline-flex;
    align-items: center;
    gap: 5px;
  }
</style>
