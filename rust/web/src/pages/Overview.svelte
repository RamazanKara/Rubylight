<script lang="ts">
  import { onMount } from 'svelte';
  import Button from '../components/Button.svelte';
  import Icon from '../components/Icon.svelte';
  import AppCover from '../components/library/AppCover.svelte';
  import { coverPath } from '../components/library/app';
  import { hostChecks } from '../lib/host-readiness';
  const landscape = new URL('../assets/home-landscape.webp', import.meta.url).href;
  import CopyButton from '../components/CopyButton.svelte';
  import PageHeader from '../components/PageHeader.svelte';
  import Panel from '../components/Panel.svelte';
  import ConnectHelp from '../components/overview/ConnectHelp.svelte';
  import ConnectionList from '../components/overview/ConnectionList.svelte';
  import HostReadiness from '../components/overview/HostReadiness.svelte';
  import RunningApp from '../components/overview/RunningApp.svelte';
  import StreamCard from '../components/overview/StreamCard.svelte';
  import { api, type App, type SessionStatus, type StreamSession } from '../lib/api';
  import { confirm, failed, notify } from '../lib/feedback.svelte';
  import { poll } from '../lib/format';
  import { link } from '../lib/router.svelte';
  import { refreshMetadata, session } from '../lib/session.svelte';

  let sessions = $state<StreamSession[] | null>(null);
  let status = $state<SessionStatus | null>(null);
  let sessionsError = $state<string | null>(null);
  let statusError = $state<string | null>(null);
  let metadataError = $state<string | null>(null);
  let rechecking = $state(false);
  let checksOpen = $state(false);
  let helpOpen = $state(false);
  let apps = $state<App[] | null>(null);
  let appsError = $state('');
  async function loadGames() {
    try { apps = (await api.apps.list()).apps; appsError = ''; }
    catch (error) { appsError = reason(error); }
  }
  /** uuid of the device being disconnected. */
  let disconnecting = $state<string | null>(null);

  const metadata = $derived(session.metadata);
  const streams = $derived((sessions ?? []).filter((item) => item.role === 'stream'));
  const others = $derived((sessions ?? []).filter((item) => item.role !== 'stream'));
  const live = $derived(streams.filter((item) => item.state === 'RUNNING').length);
  const checks = $derived(metadata ? hostChecks(metadata) : []);
  const attention = $derived(checks.some(check => check.tone === 'warn' || check.tone === 'danger'));
  const checking = $derived(!sessions || !status || !metadata || metadata.encoder_status.state === 'checking');
  const unavailable = $derived(Boolean(sessionsError || statusError || metadataError));
  const paired = $derived((metadata?.paired_devices ?? 0) > 0);
  const heading = $derived(unavailable ? 'Cannot check the host' : checking ? 'Checking this PC…' : live ? 'Streaming now' : attention ? 'This PC needs attention' : !paired ? 'Let’s connect your device' : 'Ready to play');
  const addresses = $derived(
    metadata ? (metadata.pc_addresses ?? []).filter((address) => address !== metadata.pc_address) : [],
  );

  const devices = (count: number) => `${count} ${count === 1 ? 'device' : 'devices'}`;
  const reason = (failure: unknown) => (failure instanceof Error ? failure.message : String(failure));

  const summary = $derived.by(() => {
    if (!sessions) return sessionsError ? "Can't reach the host." : 'Checking for streams…';
    const app = status?.appRunning ? status.app?.name || status.appName : '';
    if (live) return app ? `Streaming ${app} to ${devices(live)}.` : `Streaming to ${devices(live)}.`;
    if (app) return `Nothing is streaming. ${app} is ${status?.paused ? 'paused' : 'running'}.`;
    return 'Nothing is streaming.';
  });

  async function refreshSessions() {
    const [list, current] = await Promise.allSettled([api.sessions.streams(), api.sessions.status()]);
    if (list.status === 'fulfilled') {
      sessions = list.value.sessions ?? [];
      sessionsError = null;
    } else {
      sessionsError = reason(list.reason);
    }
    if (current.status === 'fulfilled') {
      status = current.value;
      statusError = null;
    } else {
      statusError = reason(current.reason);
    }
  }

  async function loadMetadata() {
    try {
      await refreshMetadata();
      metadataError = null;
    } catch (failure) {
      metadataError = reason(failure);
    }
  }

  async function recheck() {
    rechecking = true;
    await loadMetadata();
    rechecking = false;
  }

  async function disconnect(target: StreamSession) {
    const name = target.device_name || 'this device';
    const ok = await confirm({
      title: `Disconnect ${name}?`,
      message: 'Every connection from this device ends. The running app stays open, and the device can connect again.',
      confirm: 'Disconnect',
    });
    if (!ok) return;
    disconnecting = target.uuid;
    try {
      await api.clients.disconnect(target.uuid);
      notify(`Disconnected ${name}.`, 'ok');
      await refreshSessions();
    } catch (failure) {
      failed(`Disconnecting ${name} failed`, failure);
    } finally {
      disconnecting = null;
    }
  }

  onMount(() => {
    void loadGames();
    const stopSessions = poll(refreshSessions, 2000);
    const stopMetadata = poll(loadMetadata, 30_000);
    return () => {
      stopSessions();
      stopMetadata();
    };
  });
</script>

<PageHeader title="Home">
  {#snippet actions()}<Button href="/library/import" icon="plus">Add games</Button>{/snippet}
</PageHeader>

<section class="hero" style:background-image={`url(${landscape})`} aria-label="Host status">
  <div class="hero-body">
    <h2><span class="status-dot" class:ready={!unavailable && !checking && !attention} class:warn={attention && !unavailable}></span>{heading}</h2>
    <p class="hero-copy" role="status">
      {#if unavailable}The host’s latest state is unavailable. Check the connection and try again.
      {:else if checking}Checking streaming, displays, and audio…
      {:else if live}{summary}
      {:else if attention}View the checks below to get this PC ready for streaming.
      {:else if !paired}Pair your device, then choose a game in your streaming app.
      {:else}Open your streaming app on your device and choose a game.{/if}
    </p>
    <div class="hero-actions">
      {#if unavailable}<Button variant="primary" busy={rechecking} onclick={() => { void recheck(); void refreshSessions(); }}>Retry</Button>
      {:else if attention}<Button variant="primary" onclick={() => checksOpen = !checksOpen}>View checks</Button>
      {:else if !live}<Button variant="primary" href="/devices">Pair a device</Button>{/if}
      <button class="text-action" onclick={() => helpOpen = !helpOpen} aria-expanded={helpOpen}>Connection help</button>
    </div>
  </div>
  <div class="hero-foot">
    <span><Icon name="monitor" />{metadata?.host_name ?? 'This PC'}</span>
    <a href="/devices" use:link><Icon name="devices" />{metadata ? `${devices(metadata.paired_devices)} paired` : 'Checking devices…'}</a>
    <button class="text-action checks-link" onclick={() => checksOpen = !checksOpen} aria-expanded={checksOpen}>{checksOpen ? 'Hide checks' : 'View checks'}<Icon name="chevron" /></button>
  </div>
</section>

{#if sessionsError}<p class="notice danger" role="alert">{sessions ? 'Showing the last known stream state. ' : ''}{sessionsError}</p>{/if}
{#if statusError}<p class="notice danger" role="alert">Could not refresh the running app: {statusError}</p>{/if}
{#if metadataError}<p class="notice danger" role="alert">Could not refresh host details: {metadataError}</p>{/if}

{#if !paired && metadata && !checking && !unavailable}
  <ol class="setup-checklist" aria-label="Getting started">
    <li><Icon name={attention ? 'alert' : 'check'} /><div><strong>Prepare this PC</strong><span class="muted">{attention ? 'Review the host checks below.' : 'This PC is ready.'}</span></div></li>
    <li><Icon name="devices" /><div><a href="/devices" use:link>Pair a device</a><span class="muted">Enter the PIN shown on your device.</span></div></li>
    <li><Icon name="play" /><div><strong>Start playing</strong><span class="muted">Choose a game in the app on your device.</span></div></li>
  </ol>
{/if}
{#if helpOpen}<Panel title="Connect from your device"><ConnectHelp {metadata} /></Panel>{/if}
{#if checksOpen || attention}
  <HostReadiness {metadata} error={metadataError} checking={rechecking} onrecheck={recheck} />
{/if}
{#if streams.length}
  <Panel title="Connected streams" flush>
    {#each streams as stream}<StreamCard {stream} busy={disconnecting === stream.uuid} ondisconnect={disconnect} />{/each}
  </Panel>
{/if}
{#if others.length}<Panel title="Other connections" flush><ConnectionList sessions={others} busy={disconnecting} ondisconnect={disconnect} /></Panel>{/if}
{#if status?.appRunning}<RunningApp {status} error={statusError} onchange={refreshSessions} />{/if}

<section class="games" aria-label="Games & apps">
  <header><h2>Games & apps</h2><a href="/library" use:link>Manage library <Icon name="chevron" /></a></header>
  {#if appsError}<p class="notice danger" role="alert">Could not load games: {appsError}<Button onclick={loadGames}>Retry</Button></p>
  {:else if !apps}<p class="muted">Loading games…</p>
  {:else if !apps.length}<p class="empty">Your library is empty. <a href="/library/import" use:link>Add installed games</a> or <a href="/library/new" use:link>add an app manually</a>.</p>
  {:else}<div class="game-shelf">
    {#each apps.slice(0, 4) as app (app.uuid ?? app.name)}
      <a class="game" href={app.uuid ? `/library/${encodeURIComponent(app.uuid)}` : '/library'} use:link>
        <AppCover name={app.name} uuid={app.uuid} path={coverPath(app)} /><strong>{app.name}</strong>
      </a>
    {/each}
  </div><p class="shelf-hint muted">Choose a game in your streaming app to play on your device.</p>{/if}
</section>
{#if checksOpen && metadata}
  <Panel title="This PC"><dl class="facts"><dt>Address</dt><dd class="row"><span class="num">{metadata.pc_address}</span><CopyButton value={metadata.pc_address} label="Copy address" what="the address" /></dd>
    {#each addresses as address}<dt>Other address</dt><dd class="num">{address}</dd>{/each}
    <dt>Version</dt><dd>{metadata.version}</dd></dl></Panel>
{/if}

<style>
  .hero { overflow: hidden; border: 1px solid var(--media-line); border-radius: var(--radius-lg); background-color: var(--media-bg); background-size: cover; background-position: center; color: var(--media-ink); }
  .hero-body { padding: 36px; min-height: 260px; background: linear-gradient(90deg, color-mix(in srgb, var(--media-bg) 93%, transparent) 0%, color-mix(in srgb, var(--media-bg) 65%, transparent) 45%, transparent 78%); }
  .hero h2 { display: flex; align-items: center; gap: 14px; font-size: clamp(26px, 3vw, 38px); max-width: 680px; }
  .status-dot { width: 14px; height: 14px; flex: none; border-radius: 50%; background: var(--media-idle); }
  .status-dot.ready { background: var(--media-ok); }
  .status-dot.warn { background: var(--media-warn); }
  .hero-copy { max-width: 480px; font-size: 19px; color: var(--media-ink-2); margin-top: 16px; }
  .hero-actions { display: flex; flex-wrap: wrap; align-items: center; gap: 24px; margin-top: 24px; }
  .hero-actions :global(.primary) { min-width: 200px; min-height: 48px; background: var(--accent); color: var(--accent-ink); border-color: var(--accent); }
  .text-action { display: inline-flex; gap: 8px; align-items: center; padding: 10px 0; min-height: 44px; border: 0; background: none; color: inherit; text-decoration: underline; text-underline-offset: 4px; cursor: pointer; }
  .hero-foot { display: flex; align-items: center; flex-wrap: wrap; gap: 12px 28px; padding: 10px 32px; background: color-mix(in srgb, var(--media-bg) 80%, transparent); border-top: 1px solid var(--media-line); font-size: 14px; }
  .hero-foot > span, .hero-foot > a { display: inline-flex; align-items: center; gap: 10px; }
  .hero-foot > a { text-decoration: none; }
  .checks-link { margin-left: auto; }
  .setup-checklist { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 24px; list-style: none; margin: 0; padding: 24px; background: var(--panel); border: 1px solid var(--line); border-radius: var(--radius-lg); }
  .setup-checklist li { display: flex; gap: 12px; }
  .setup-checklist li :global(svg) { flex: none; margin-top: 3px; }
  .setup-checklist li div { display: grid; gap: 4px; }
  .setup-checklist span { font-size: var(--text-sm); }
  .games { display: grid; gap: 16px; }
  .games header { display: flex; align-items: center; justify-content: space-between; gap: 16px; flex-wrap: wrap; }
  .games h2 { font-size: 28px; }
  .games header a { display: inline-flex; align-items: center; gap: 8px; min-height: 44px; text-decoration: none; }
  .game-shelf { display: grid; grid-template-columns: repeat(4, minmax(0, 1fr)); gap: 16px; }
  .game { display: grid; align-content: start; gap: 10px; text-decoration: none; min-width: 0; }
  .game:hover { color: var(--ink-2); }
  .game :global(.cover) { aspect-ratio: 4 / 5; }
  .shelf-hint { padding-top: 16px; border-top: 1px solid var(--line); font-size: var(--text-sm); }
  .empty { padding: 24px; border: 1px dashed var(--line-strong); border-radius: var(--radius-lg); }
  @media (max-width: 700px) { .hero-body { padding: 24px; } .hero-foot { padding: 12px 24px; gap: 6px 16px; } .setup-checklist { grid-template-columns: 1fr; } .game-shelf { grid-template-columns: repeat(2, minmax(0, 1fr)); } .checks-link { margin-left: 0; } }
</style>
