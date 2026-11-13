<script lang="ts">
  import { onMount } from 'svelte';
  import Button from '../components/Button.svelte';
  import Icon from '../components/Icon.svelte';
  import AppCover from '../components/library/AppCover.svelte';
  import CoopCard from '../components/overview/CoopCard.svelte';
  import GuestsCard from '../components/overview/GuestsCard.svelte';
  import { coverPath, coverVersion, placeholder } from '../components/library/app';
  import { homeLead, problems } from '../lib/host-readiness';
  import { applyFix, dismissWarning, fixes } from '../lib/host-fixes.svelte';
  import CopyButton from '../components/CopyButton.svelte';
  import PageHeader from '../components/PageHeader.svelte';
  import Panel from '../components/Panel.svelte';
  import ConnectHelp from '../components/overview/ConnectHelp.svelte';
  import PairPanel from '../components/devices/PairPanel.svelte';
  import ConnectionList from '../components/overview/ConnectionList.svelte';
  import HostReadiness from '../components/overview/HostReadiness.svelte';
  import RunningApp from '../components/overview/RunningApp.svelte';
  import Sparkline from '../components/overview/Sparkline.svelte';
  import StreamCard from '../components/overview/StreamCard.svelte';
  import { heroGame, liveFigures } from '../components/overview/now-playing';
  import {
    api, type App, type Client, type CoopStatus, type PowerStatus, type SessionGuestsStatus, type SessionStatus,
    type StreamSession,
  } from '../lib/api';
  import { shown as guestsShown } from '../lib/session-guests';
  import { endedTitle, endedTone, endText, homeSessions, lastEnded, liveCoop } from '../lib/coop-view';
  import Banner from '../components/Banner.svelte';
  import { confirm, failed, notify } from '../lib/feedback.svelte';
  import { duration, marked, poll } from '../lib/format';
  import { checkPowerStatus } from '../lib/power';
  import { link } from '../lib/router.svelte';
  import { refreshMetadata, session } from '../lib/session.svelte';

  // The art Home shows before a device is paired, and when no game has been played.
  const landscape = new URL('../assets/home-landscape.webp', import.meta.url).href;

  let sessions = $state<StreamSession[] | null>(null);
  let status = $state<SessionStatus | null>(null);
  let sessionsError = $state<string | null>(null);
  let statusError = $state<string | null>(null);
  let metadataError = $state<string | null>(null);
  let rechecking = $state(false);
  let detailsOpen = $state(false);
  let helpOpen = $state(false);
  let apps = $state<App[] | null>(null);
  let appsError = $state('');
  async function loadGames() {
    try { apps = (await api.apps.list()).apps; appsError = ''; }
    catch (error) { appsError = reason(error); }
  }
  /** uuid of the device being disconnected. */
  let disconnecting = $state<string | null>(null);
  /** The device paired while this page was open: the pairing stays on top with its card. */
  let welcomed = $state<Client | null>(null);

  // Whether devices can wake this PC, once one is paired: read on arrival and on Recheck, never polled.
  let power = $state<PowerStatus | null>(null);
  let powerAsked = false;
  async function loadPower() {
    try { power = checkPowerStatus(await api.power.status()); }
    catch { power = null; }
  }

  // Live co-op games: one card each, with the paired devices to invite while one runs.
  let coop = $state<CoopStatus | null>(null);
  let coopAt = $state(0);
  let coopError = $state('');
  let coopClients = $state<Client[] | null>(null);
  let coopClientsError = $state('');
  const coopGames = $derived(homeSessions(coop));
  const coopEnded = $derived(lastEnded(coop));
  /** The ended session whose banner was dismissed. */
  let coopDismissed = $state<number | null>(null);
  async function refreshCoop() {
    try {
      coop = await api.coop.status();
      coopAt = Date.now();
      coopError = '';
    } catch (error) {
      // A host without co-op answers nothing to show; a card already shown says it is stale.
      coopError = reason(error);
      return;
    }
    if (!liveCoop(coop).length) return;
    try {
      // After the status's await, so marked here: the poll's own mark covers only its first request.
      coopClients = (await marked(() => api.clients.list())).clients;
      coopClientsError = '';
    } catch (error) {
      coopClientsError = reason(error);
    }
  }

  const metadata = $derived(session.metadata);
  const streams = $derived((sessions ?? []).filter((item) => item.role === 'stream'));
  const others = $derived((sessions ?? []).filter((item) => item.role !== 'stream'));
  const live = $derived(streams.filter((item) => item.state === 'RUNNING').length);
  const unavailable = $derived(Boolean(sessionsError || statusError || metadataError));
  const paired = $derived((metadata?.paired_devices ?? 0) > 0);
  const wakePower = $derived(paired ? power : null);
  // What needs a look, ranked: the hero leads with the first and its button fixes it.
  const found = $derived(metadata ? problems(metadata, wakePower, fixes.dismissed) : []);
  const top = $derived(found[0]);
  const lead = $derived(homeLead({
    unavailable,
    loaded: Boolean(sessions && status && metadata),
    live: live > 0,
    paired,
    probing: metadata?.encoder_status.state === 'checking',
    top,
  }));
  /** Nothing paired yet: pairing is the top of Home, and stays there with the new device's card once one pairs. */
  const firstRun = $derived(Boolean(metadata) && !unavailable && !live && !top?.blocking && (!paired || welcomed !== null));
  const heroProblem = $derived(lead === 'blocked' || lead === 'problem' ? top : undefined);
  /** The checks list itself: for the problems the hero doesn't name, or on request. */
  const showChecks = $derived(detailsOpen || found.length > (heroProblem ? 1 : 0));
  // A stream of the desktop shows no last-played game as if it were playing.
  const game = $derived.by(() => {
    const current = paired ? heroGame(apps, status) : null;
    return lead === 'live' && current?.state === 'last' ? null : current;
  });
  const figures = $derived(lead === 'live' ? liveFigures(streams) : null);
  const addresses = $derived(
    metadata ? (metadata.pc_addresses ?? []).filter((address) => address !== metadata.pc_address) : [],
  );

  const devices = (count: number) => `${count} ${count === 1 ? 'device' : 'devices'}`;
  const reason = (failure: unknown) => (failure instanceof Error ? failure.message : String(failure));

  const heading = $derived.by(() => {
    if (lead === 'unavailable') return 'Can’t reach this PC';
    if (lead === 'checking') return 'Checking this PC…';
    if (lead === 'live') return figures ? `Streaming to ${figures.device}` : 'Streaming now';
    if (heroProblem) return heroProblem.headline;
    return 'Ready to play';
  });
  const copy = $derived.by(() => {
    if (lead === 'unavailable') return 'Its latest state is unavailable.';
    if (lead === 'checking') return 'Checking streaming, displays and audio…';
    if (lead === 'live') return figures?.more ? `And to ${devices(figures.more)} more.` : '';
    if (heroProblem) return heroProblem.text;
    if (game && game.state !== 'last') return `${game.name} is ${game.state === 'paused' ? 'paused' : 'running'} on this PC.`;
    return '';
  });
  const gameLabel = $derived(
    !game ? '' : lead === 'live' ? 'Now playing' : game.state === 'paused' ? 'Paused' : game.state === 'running' ? 'Running' : 'Last played',
  );
  /** The hero's art: the game's cover, blurred behind the text, or its tint without one; the landscape without a game. */
  const art = $derived.by(() => {
    if (!game) return { image: landscape, blur: false, hue: 0 };
    const hue = placeholder(game.name).hue;
    const path = game.app ? coverPath(game.app) : '';
    const uuid = game.app?.uuid;
    return { image: uuid && path ? api.apps.coverUrl(uuid, coverVersion(path)) : '', blur: true, hue };
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
    await Promise.all([loadMetadata(), paired ? loadPower() : undefined]);
    rechecking = false;
  }

  /** A fix finished: a wake fix brings the new wake status; a settings fix refreshed the details itself. */
  function fixed(next: PowerStatus | null) {
    if (next) power = next;
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

  // Guests from paired devices in the running game, and devices waiting to join it.
  let deviceGuests = $state<SessionGuestsStatus | null>(null);
  async function refreshDeviceGuests() {
    try {
      deviceGuests = await api.sessionGuests.status();
    } catch {
      // A host without guests answers nothing to show.
      deviceGuests = null;
    }
  }

  $effect(() => {
    if (!paired || powerAsked) return;
    powerAsked = true;
    void loadPower();
  });

  onMount(() => {
    void loadGames();
    const stopSessions = poll(refreshSessions, 2000);
    const stopMetadata = poll(loadMetadata, 30_000);
    const stopCoop = poll(refreshCoop, 3000);
    const stopGuests = poll(refreshDeviceGuests, 2000);
    return () => {
      stopSessions();
      stopMetadata();
      stopCoop();
      stopGuests();
    };
  });
</script>

<PageHeader title="Home" />

{#snippet helpToggle()}
  <button class="text-action" onclick={() => (helpOpen = !helpOpen)} aria-expanded={helpOpen}>Connection help</button>
{/snippet}

{#if firstRun}
  <!-- Nothing paired: the top is pairing, its two ways side by side. -->
  <section class="hero first" aria-label="PC status">
    <div class="backdrop" style:background-image={`url(${landscape})`} aria-hidden="true"></div>
    <div class="hero-main">
      <div class="hero-body">
        <h2><span class="status-dot" class:ready={welcomed}></span>{welcomed ? 'Ready to play' : 'Let’s connect your device'}</h2>
        <p class="hero-copy">{welcomed ? `${welcomed.name} is paired and can play your games.` : 'Play your PC games on a phone, tablet, TV or another PC.'}</p>
        <div class="hero-actions">
          {#if welcomed}<Button variant="primary" href="/library">See your games</Button>{/if}
          {@render helpToggle()}
        </div>
      </div>
    </div>
    <div class="pair"><PairPanel bare onpaired={(client) => (welcomed = client)} /></div>
  </section>
{:else}
  <section class="hero" class:art={!art.blur} aria-label="PC status">
    <div class="backdrop" class:blur={art.blur} class:tint={art.blur && !art.image} style:--hue={art.hue}
      style:background-image={art.image ? `url("${art.image}")` : undefined} aria-hidden="true"></div>
    <div class="hero-main">
      <div class="hero-body">
        <h2><span class="status-dot" class:ready={lead === 'ready' || lead === 'live'} class:warn={heroProblem || lead === 'unavailable'}></span>{heading}</h2>
        {#if copy}<p class="hero-copy" role="status">{copy}</p>{/if}
        {#if figures}
          <div class="live" role="group" aria-label="Live stream">
            <span class="stat"><strong class="num">{figures.fps ?? '–'}</strong> fps</span>
            <span class="stat"><strong class="num">{figures.mbps == null ? '–' : figures.mbps.toFixed(1)}</strong> Mbps</span>
            {#if figures.trend.length > 1}
              <span class="spark"><Sparkline values={figures.trend} label="Bitrate over the last {duration(figures.trend.length)}" /></span>
            {/if}
          </div>
        {/if}
        {#if lead !== 'live'}<div class="hero-actions">
          {#if lead === 'unavailable'}
            <Button variant="primary" busy={rechecking} onclick={() => { void recheck(); void refreshSessions(); }}>Retry</Button>
          {:else if heroProblem?.fix}
            <Button variant="primary" busy={fixes.running === heroProblem.id} disabled={Boolean(fixes.running)} onclick={() => heroProblem && applyFix(heroProblem, fixed)}>
              {heroProblem.fix.label}<span class="visually-hidden">: {heroProblem.title}</span>
            </Button>
          {:else if heroProblem?.href}
            <Button variant="primary" href={heroProblem.href}>{heroProblem.settings}</Button>
          {:else if heroProblem?.warning}
            <Button variant="primary" busy={fixes.running === heroProblem.id} disabled={Boolean(fixes.running)} onclick={() => heroProblem && dismissWarning(heroProblem, recheck)}>Dismiss</Button>
          {:else if lead === 'ready'}
            <Button variant="primary" href="/library">See your games</Button>
          {/if}
          {@render helpToggle()}
        </div>{/if}
      </div>
      {#if game}
        <a class="now" href={game.app?.uuid ? `/library/${encodeURIComponent(game.app.uuid)}` : '/library'} use:link>
          <span class="art-frame"><AppCover name={game.name} uuid={game.app?.uuid} path={game.app ? coverPath(game.app) : ''} /></span>
          <span class="now-text"><span class="now-label">{gameLabel}</span><strong>{game.name}</strong></span>
        </a>
      {/if}
    </div>
    <div class="hero-foot">
      <span class="fact"><Icon name="monitor" />{metadata?.host_name ?? 'This PC'}</span>
      <a class="fact" href="/devices" use:link><Icon name="devices" />{metadata ? `${devices(metadata.paired_devices)} paired` : 'Checking devices…'}</a>
      <button class="text-action details-link" class:open={detailsOpen} onclick={() => (detailsOpen = !detailsOpen)} aria-expanded={detailsOpen}>{detailsOpen ? 'Hide PC details' : 'PC details'}<Icon name="chevron" size={16} /></button>
    </div>
  </section>
{/if}

{#if sessionsError}<p class="notice danger" role="alert">{sessions ? 'Showing the last known stream state. ' : ''}{sessionsError}</p>{/if}
{#if statusError}<p class="notice danger" role="alert">Could not refresh the running app: {statusError}</p>{/if}
{#if metadataError}<p class="notice danger" role="alert">Could not refresh this PC's details: {metadataError}</p>{/if}

{#if showChecks}
  <div id="checks" class="anchor"><HostReadiness {metadata} power={wakePower} error={metadataError} checking={rechecking} onrecheck={recheck} onfixed={fixed}
    skip={detailsOpen ? undefined : heroProblem?.id} /></div>
{/if}
{#if detailsOpen && metadata}
  <Panel title="This PC"><dl class="facts"><dt>Address</dt><dd class="row"><span class="num">{metadata.pc_address}</span><CopyButton value={metadata.pc_address} label="Copy address" what="the address" /></dd>
    {#each addresses as address}<dt>Other address</dt><dd class="num">{address}</dd>{/each}
    <dt>Version</dt><dd>{metadata.version}</dd></dl></Panel>
{/if}
{#if helpOpen}<Panel title="Connect from your device"><ConnectHelp {metadata} /></Panel>{/if}
{#if !firstRun && !paired && metadata && !unavailable}<PairPanel />{/if}
{#if coopError && coopGames.length}<p class="notice danger" role="alert">Showing the last known co-op state. {coopError}</p>{/if}
{#if coopEnded?.end && coopEnded.id !== coopDismissed}
  <Banner tone={endedTone(coopEnded.end)} title={endedTitle(coopEnded)} live ondismiss={() => (coopDismissed = coopEnded.id)}>
    {endText(coopEnded.end)}
  </Banner>
{/if}
{#each coopGames as coopGame (coopGame.id)}
  <CoopCard session={coopGame} clients={coopClients} clientsError={coopClientsError} at={coopAt} onchange={refreshCoop} />
{/each}
{#if deviceGuests && guestsShown(deviceGuests)}
  <GuestsCard status={deviceGuests} onchange={(next) => (deviceGuests = next)} />
{/if}
{#if streams.length}
  <Panel title="Connected streams" flush>
    {#each streams as stream}<StreamCard {stream} busy={disconnecting === stream.uuid} ondisconnect={disconnect} />{/each}
  </Panel>
{/if}
{#if others.length}<Panel title="Other connections" flush><ConnectionList sessions={others} busy={disconnecting} ondisconnect={disconnect} /></Panel>{/if}
{#if status?.appRunning}<RunningApp {status} error={statusError} onchange={refreshSessions} />{/if}

<section class="games" aria-labelledby="games-title">
  <header><h2 id="games-title">Your games</h2><a href="/library" use:link>All games <Icon name="chevron" /></a></header>
  {#if appsError}<div class="notice danger games-error" role="alert"><span>Could not load games: {appsError}</span><Button size="sm" onclick={loadGames}>Retry</Button></div>
  {:else if !apps}<p class="muted">Loading games…</p>
  {:else if !apps.length}<p class="empty">No games yet. Games from Steam and your other launchers appear here by themselves. <a href="/library" use:link>Launchers</a></p>
  {:else}<div class="game-shelf">
    {#each apps.slice(0, 6) as app (app.uuid ?? app.name)}
      <a class="game" href={app.uuid ? `/library/${encodeURIComponent(app.uuid)}` : '/library'} use:link>
        <span class="art-frame"><AppCover name={app.name} uuid={app.uuid} path={coverPath(app)} /></span><strong class="name">{app.name}</strong>
      </a>
    {/each}
  </div>{/if}
</section>

<style>
  .anchor { display: grid; scroll-margin-top: var(--space-4); }

  /* The top: status over art, dark in both themes like game art. */
  .hero { position: relative; isolation: isolate; overflow: hidden; border: 1px solid var(--media-line); border-radius: var(--radius-lg); background: var(--media-bg); color: var(--media-ink); box-shadow: var(--shadow); }
  .hero-main :focus-visible, .hero-foot :focus-visible { outline-color: var(--media-ink); }
  .backdrop { position: absolute; inset: 0; z-index: -1; background-color: var(--media-bg); background-size: cover; background-position: center; }
  /* A cover is a poster: blurred to a wash of its colours behind the text, sharp in its frame. */
  .backdrop.blur { inset: -48px; filter: blur(32px) saturate(1.3); opacity: 0.9; }
  .backdrop.tint {
    background-image:
      radial-gradient(130% 90% at 85% 10%, oklch(from var(--media-bg) calc(l + 0.26) 0.09 var(--hue) / 0.85), transparent 70%),
      linear-gradient(165deg, oklch(from var(--media-bg) calc(l + 0.14) 0.06 var(--hue)), oklch(from var(--media-bg) calc(l + 0.02) 0.03 var(--hue)));
  }
  .hero-main { display: grid; grid-template-columns: minmax(0, 1fr) auto; align-items: center; gap: var(--space-5) var(--space-6); padding: var(--space-6); min-height: 240px;
    background: linear-gradient(90deg, color-mix(in srgb, var(--media-bg) 92%, transparent) 0%, color-mix(in srgb, var(--media-bg) 74%, transparent) 55%, color-mix(in srgb, var(--media-bg) 60%, transparent) 100%); }
  /* The landscape is the picture itself: the scrim only holds the text. */
  .hero.art .hero-main, .first .hero-main { background: linear-gradient(90deg, color-mix(in srgb, var(--media-bg) 93%, transparent) 0%, color-mix(in srgb, var(--media-bg) 65%, transparent) 45%, transparent 78%); }
  .hero-body { display: grid; align-content: center; justify-items: start; gap: var(--space-3); min-width: 0; }
  .hero h2 { display: flex; align-items: center; gap: var(--space-3); font-size: clamp(26px, 3vw, 36px); letter-spacing: -0.01em; max-width: 680px; }
  .status-dot { width: 12px; height: 12px; flex: none; border-radius: 50%; --dot: var(--media-idle); background: var(--dot); box-shadow: 0 0 0 4px color-mix(in srgb, var(--dot) 22%, transparent); }
  .status-dot.ready { --dot: var(--media-ok); }
  .status-dot.warn { --dot: var(--media-warn); }
  .hero-copy { max-width: 520px; font-size: var(--text-md); color: var(--media-ink-2); }
  .hero-actions { display: flex; flex-wrap: wrap; align-items: center; gap: var(--space-2) var(--space-5); margin-top: var(--space-2); }
  .hero-actions :global(.primary) { min-width: 168px; min-height: 48px; padding-inline: var(--space-5); font-size: var(--text); }
  .text-action { display: inline-flex; gap: var(--space-1); align-items: center; padding: 10px 2px; min-height: var(--control-height); border: 0; border-radius: var(--radius-sm); background: none; color: inherit; font-weight: var(--weight-medium); text-decoration: underline; text-decoration-color: color-mix(in srgb, currentColor 45%, transparent); text-underline-offset: 4px; cursor: pointer; transition: text-decoration-color var(--duration-fast); }
  .text-action:hover { text-decoration-color: currentColor; }

  /* While streaming: the device's live rate and bitrate. */
  .live { display: flex; flex-wrap: wrap; align-items: center; gap: var(--space-2) var(--space-5); color: var(--media-ink-2); }
  .stat strong { font-family: var(--font-display); font-size: var(--text-xl); font-weight: var(--weight-bold); color: var(--media-ink); }
  .spark { width: 200px; --ink-2: var(--media-ink); --line: var(--media-line); }

  /* The game: its cover, like the library's, with what it is to Home. */
  .now { display: grid; gap: var(--space-2); width: 148px; color: inherit; text-decoration: none; border-radius: var(--radius); }
  .art-frame { display: block; border-radius: var(--radius); transition: transform var(--duration-fast) var(--ease-standard), box-shadow var(--duration-fast) var(--ease-standard); }
  .now .art-frame { box-shadow: 0 8px 24px rgb(0 0 0 / 35%); }
  .now:hover .art-frame, .game:hover .art-frame { transform: translateY(-2px); box-shadow: var(--shadow-raised); }
  .now:hover .art-frame { box-shadow: 0 10px 28px rgb(0 0 0 / 45%); }
  .now-text { display: grid; gap: 2px; min-width: 0; }
  .now-label { font-size: var(--text-xs); font-weight: var(--weight-semibold); letter-spacing: 0.04em; text-transform: uppercase; color: var(--media-ink-2); }
  .now strong { font-size: var(--text-sm); font-weight: var(--weight-semibold); line-height: var(--leading-snug); overflow-wrap: anywhere; }
  .now:hover strong { text-decoration: underline; text-underline-offset: 3px; }

  .hero-foot { display: flex; align-items: center; flex-wrap: wrap; gap: 0 var(--space-5); padding: 0 var(--space-6); background: color-mix(in srgb, var(--media-bg) 82%, transparent); border-top: 1px solid var(--media-line); font-size: var(--text-sm); color: var(--media-ink-2); }
  .fact { display: inline-flex; align-items: center; gap: var(--space-2); min-height: var(--control-height); }
  .fact :global(svg) { color: var(--media-idle); }
  a.fact { text-decoration: none; border-radius: var(--radius-sm); }
  a.fact:hover { color: var(--media-ink); text-decoration: underline; text-underline-offset: 4px; }
  .details-link { margin-left: auto; color: var(--media-ink); }
  .details-link :global(svg) { transition: transform var(--duration-fast) var(--ease-standard); }
  .details-link.open :global(svg) { transform: rotate(90deg); }

  /* First run: a short band of art over the two ways to pair. */
  .first .hero-main { min-height: 0; grid-template-columns: minmax(0, 1fr); }
  .pair { padding: var(--space-5) var(--space-6) var(--space-6); background: var(--panel); color: var(--ink); border-top: 1px solid var(--media-line); }

  .games-error { align-items: center; justify-content: space-between; flex-wrap: wrap; }
  .games { display: grid; gap: var(--space-4); }
  .games header { display: flex; align-items: center; justify-content: space-between; gap: var(--space-4); flex-wrap: wrap; }
  .games h2 { font-size: var(--text-xl); }
  .games header a { display: inline-flex; align-items: center; gap: var(--space-2); min-height: var(--control-height); text-decoration: none; }
  /* The library's covers: 2:3, lifted on hover, the name in the accent. */
  .game-shelf { display: grid; grid-template-columns: repeat(6, minmax(0, 1fr)); gap: var(--space-5) var(--space-4); }
  .game { display: grid; align-content: start; gap: var(--space-2); min-width: 0; color: inherit; text-decoration: none; border-radius: var(--radius); }
  .name { font-weight: var(--weight-medium); font-size: var(--text-sm); line-height: var(--leading-snug); overflow-wrap: anywhere; display: -webkit-box; -webkit-box-orient: vertical; -webkit-line-clamp: 2; line-clamp: 2; overflow: hidden; transition: color var(--duration-fast) var(--ease-standard); }
  .game:hover .name { color: var(--accent-text); }
  .empty { padding: var(--space-5); border: 1px dashed var(--line-strong); border-radius: var(--radius-lg); }

  @media (max-width: 1100px) {
    .game-shelf { grid-template-columns: repeat(4, minmax(0, 1fr)); }
    .game:nth-child(n + 5) { display: none; }
  }
  @media (max-width: 700px) {
    /* A phone stacks the text over the art: one even scrim keeps all of it readable. */
    .hero-main, .hero.art .hero-main, .first .hero-main { grid-template-columns: minmax(0, 1fr); padding: var(--space-5); min-height: 0; background: color-mix(in srgb, var(--media-bg) 80%, transparent); }
    .hero-actions { flex-direction: column; align-items: stretch; justify-self: stretch; gap: var(--space-1); }
    .hero-actions .text-action { align-self: flex-start; }
    .now { width: auto; grid-template-columns: 64px minmax(0, 1fr); align-items: center; gap: var(--space-3); }
    .hero-foot { padding: var(--space-1) var(--space-5); gap: 0 var(--space-4); }
    .details-link { margin-left: 0; }
    .pair { padding: var(--space-4); }
  }
  @media (max-width: 520px) {
    .game-shelf { grid-template-columns: repeat(3, minmax(0, 1fr)); gap: var(--space-4) var(--space-3); }
    .game:nth-child(n + 5) { display: grid; }
  }
</style>
