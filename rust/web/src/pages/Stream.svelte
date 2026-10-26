<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import Badge from '../components/Badge.svelte';
  import Button from '../components/Button.svelte';
  import EmptyState from '../components/EmptyState.svelte';
  import PageHeader from '../components/PageHeader.svelte';
  import Panel from '../components/Panel.svelte';
  import { api, sessionCsrfToken } from '../lib/api';
  import { link } from '../lib/router.svelte';
  import { on } from '../lib/schema/basics';
  import { ConsoleStream, STREAM, type AppEntry } from '../lib/webrtc/client';
  import { buttonChanges, keyMessage, positionMessage, Wheel } from '../lib/webrtc/input';
  import { RefusedError } from '../lib/webrtc/signalling';

  // The socket opens on the click to connect, not with the page: one tab or browser holds it,
  // and a second one takes it over.
  type Phase = 'checking' | 'off' | 'idle' | 'connecting' | 'ready' | 'starting' | 'streaming' | 'failed';

  let phase = $state<Phase>('checking');
  let error = $state('');
  let refused = $state(false);
  let signedOut = $state(false);
  let status = $state('');
  let quitting = $state(false);
  /** The settings have been read, so a failure is the socket's. */
  let checked = $state(false);
  let anyNetwork = $state(false);
  let apps = $state.raw<AppEntry[]>([]);
  let title = $state('');
  let video = $state<HTMLVideoElement>();
  let surface = $state<HTMLDivElement>();
  let stream: ConsoleStream | null = null;
  let gone = false;
  /** The mouse buttons the host holds, as a `MouseEvent.buttons` mask. */
  let buttons = 0;
  let wheelInput = new Wheel();

  const live = $derived(phase === 'starting' || phase === 'streaming');

  const message = (cause: unknown) => (cause instanceof Error ? cause.message : String(cause));

  async function check() {
    try {
      const config = await api.config.get();
      anyNetwork = String(config.origin_web_ui_allowed ?? '').trim() === 'wan';
      phase = on(config, 'webrtc_server', false) ? 'idle' : 'off';
      checked = true;
    } catch (cause) {
      error = message(cause);
      phase = 'failed';
    }
  }

  async function connect() {
    stream?.close();
    stream = null;
    phase = 'connecting';
    error = '';
    refused = false;
    signedOut = false;
    try {
      const opened = await ConsoleStream.connect(await sessionCsrfToken());
      if (gone) return opened.close();
      stream = opened;
      opened.onended = (reason, closed) => ended(opened, reason, closed);
      apps = await opened.apps();
      if (stream === opened) phase = 'ready';
    } catch (cause) {
      if (gone) return;
      refused = cause instanceof RefusedError;
      error = message(cause);
      phase = 'failed';
      // A lapsed sign-in gets an anonymous token, which the host refuses.
      if (refused) signedOut = !(await api.auth.status().catch(() => ({ authenticated: true }))).authenticated;
    }
  }

  /** The stream ended on its own; `closed` when the socket went with it. */
  function ended(from: ConsoleStream, reason: string, closed: boolean) {
    if (stream !== from || gone) return;
    error = reason;
    status = '';
    if (document.fullscreenElement) void document.exitFullscreen();
    if (closed) {
      stream = null;
      phase = 'idle';
      return;
    }
    if (live) phase = 'ready';
    void refreshApps();
  }

  async function refreshApps() {
    const current = stream;
    if (!current) return;
    try {
      apps = await current.apps();
    } catch (cause) {
      // A closed socket has gone through `ended`, which says why.
      if (stream === current && !current.closed) error = message(cause);
    }
  }

  async function start(app: AppEntry) {
    const current = stream;
    if (!current || !video || phase !== 'ready') return;
    error = '';
    title = app.title;
    phase = 'starting';
    buttons = 0;
    wheelInput = new Wheel();
    // Called before the first await: `video.play()` needs the click's user gesture in Safari.
    const started = current.start(app.id, video, (text) => (status = text));
    try {
      await started;
      if (stream !== current || phase !== 'starting') return;
      phase = 'streaming';
      surface?.focus();
    } catch (cause) {
      // Stopped by the page, or ended by the host: the page or `ended` has said so already.
      if (stream !== current || phase !== 'starting') return;
      error = message(cause);
      status = '';
      phase = 'ready';
      void refreshApps();
    }
  }

  function leave() {
    status = '';
    if (document.fullscreenElement) void document.exitFullscreen();
    phase = 'ready';
  }

  function disconnect() {
    stream?.disconnect();
    leave();
    void refreshApps();
  }

  async function quit() {
    const current = stream;
    if (!current) return;
    leave();
    quitting = true;
    try {
      await current.quit();
    } catch (cause) {
      if (stream === current) error = message(cause);
    } finally {
      quitting = false;
    }
    void refreshApps();
  }

  function fullscreen() {
    surface?.requestFullscreen().catch((cause: unknown) => (error = `Full screen did not open: ${message(cause)}`));
    surface?.focus();
  }

  // Input goes to the host only while the picture has the focus.
  function key(event: KeyboardEvent, down: boolean) {
    if (phase !== 'streaming') return;
    // Ctrl+Alt+Shift+Z gives the keyboard back to the page, as in Moonlight.
    if (down && event.ctrlKey && event.altKey && event.shiftKey && event.code === 'KeyZ') {
      surface?.blur();
      return;
    }
    event.preventDefault();
    stream?.send(keyMessage(event.code, down));
  }

  function position(event: PointerEvent) {
    if (phase !== 'streaming' || !video) return;
    const size = { width: video.videoWidth, height: video.videoHeight };
    stream?.send(positionMessage(event.clientX, event.clientY, video.getBoundingClientRect(), size));
  }

  /** Follows the buttons mask on every pointer event: a second button arrives as a move. */
  function pointer(event: PointerEvent) {
    if (phase !== 'streaming') return;
    if (event.type === 'pointerdown') {
      event.preventDefault();
      surface?.focus();
      surface?.setPointerCapture(event.pointerId);
    }
    position(event);
    pressed(event.buttons);
  }

  function pressed(mask: number) {
    for (const change of buttonChanges(buttons, mask)) stream?.send(change);
    buttons = mask;
  }

  function wheel(event: WheelEvent) {
    if (phase !== 'streaming') return;
    event.preventDefault();
    stream?.send(wheelInput.message(event.deltaX, event.deltaY, event.deltaMode));
  }

  function blurred() {
    stream?.releaseAll();
    buttons = 0;
  }

  onMount(() => void check());
  onDestroy(() => {
    gone = true;
    stream?.close();
    stream = null;
  });
</script>

<div class="page">
  <PageHeader title="Stream" subtitle="Play a game from this PC in this browser tab: H.264 at {STREAM.width}×{STREAM.height} and {STREAM.fps} fps, stereo sound, keyboard and mouse.">
    {#snippet actions()}
      <Badge tone="accent">Experimental</Badge>
    {/snippet}
  </PageHeader>

  {#if anyNetwork}
    <p class="notice warn">Console access is set to any network, but streaming works on the local network and Tailscale only.</p>
  {/if}

  {#if phase === 'checking'}
    <p class="muted" role="status">Checking the host…</p>
  {:else if phase === 'connecting'}
    <p class="muted" role="status">Connecting to the host…</p>
  {:else if phase === 'off'}
    <EmptyState icon="monitor" title="The WebRTC server is off">
      <p>Turn on <strong>WebRTC server</strong> in <a href="/settings/network" use:link>Settings, Network</a>, then restart Rubylight.</p>
    </EmptyState>
  {:else if phase === 'failed'}
    <div class="notice danger failure" role="alert">
      <div class="stack">
        {#if signedOut}
          <strong>This tab is no longer signed in.</strong>
          <span><a href="/login?next=%2Fstream">Sign in again</a> to stream.</span>
        {:else if refused}
          <strong>The host refused the stream connection.</strong>
          <span>The browser does not say why. One of these is the cause:</span>
          <ul>
            <li>This tab is not signed in, or it opened the console from an address the host does not list as its own (see <code>csrf_allowed_origins</code>).</li>
            <li>This device is not on the local network or Tailscale.</li>
            <li>The WebRTC server is off, or was turned on without restarting Rubylight.</li>
            <li>Your console device, “Console ({'<'}your user{'>'})”, is disabled on <a href="/devices" use:link>Devices</a>.</li>
          </ul>
        {:else if checked}
          <span>The stream connection failed: {error}</span>
        {:else}
          <span>The host's settings did not load: {error}</span>
        {/if}
      </div>
      <Button size="sm" icon="refresh" onclick={checked ? connect : check}>Try again</Button>
    </div>
  {:else}
    {#if error}
      <p class="notice danger" role="alert">{error}</p>
    {/if}
    {#if quitting}
      <p class="muted" role="status">Quitting the app…</p>
    {/if}
    {#if phase === 'idle'}
      <Panel title="Games & apps" description="Connect to list them. One tab or browser streams at a time: connecting here takes over from another.">
        <div><Button icon="play" onclick={connect}>Connect</Button></div>
      </Panel>
    {:else if phase === 'ready'}
      <Panel title="Games & apps" description="Pick one to start it on this PC and stream it here." flush>
        {#if apps.length}
          <ul class="apps">
            {#each apps as app (app.id)}
              <li>
                <span class="name">{app.title}</span>
                {#if app.running}<Badge tone="live">Running</Badge>{/if}
                <Button size="sm" icon="play" disabled={quitting} onclick={() => start(app)}>Stream<span class="visually-hidden"> {app.title}</span></Button>
              </li>
            {/each}
          </ul>
        {:else}
          <EmptyState icon="library" title="No games or apps">
            <p>Add one in <a href="/library" use:link>Games & apps</a>.</p>
          </EmptyState>
        {/if}
      </Panel>
    {/if}
  {/if}

  <section class="player" hidden={!live} aria-label="Stream of {title}">
    <div class="bar">
      <span class="name">{title}</span>
      {#if status}<span class="muted" role="status">{status}</span>{/if}
      <span class="spacer"></span>
      <Button size="sm" icon="monitor" disabled={phase !== 'streaming'} onclick={fullscreen}>Full screen</Button>
      <Button size="sm" variant="secondary" onclick={disconnect}>Disconnect</Button>
      <Button size="sm" variant="danger" icon="stop" onclick={quit}>Quit app</Button>
    </div>
    <!-- The picture takes every key and pointer event while it has the focus, as a game does. -->
    <!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
    <div
      class="surface"
      bind:this={surface}
      tabindex="0"
      role="application"
      aria-label="Stream picture. Keys and the mouse go to the host while it has the focus; Ctrl+Alt+Shift+Z gives the keyboard back."
      onkeydown={(event) => key(event, true)}
      onkeyup={(event) => key(event, false)}
      onpointermove={pointer}
      onpointerdown={pointer}
      onpointerup={pointer}
      onpointercancel={() => pressed(0)}
      onwheel={wheel}
      oncontextmenu={(event) => event.preventDefault()}
      onblur={blurred}
    >
      <video id="stream-video" bind:this={video} autoplay playsinline></video>
    </div>
    <p class="muted hint">Click the picture to play. Keys and the mouse go to the host while it has the focus; click outside it or press Ctrl+Alt+Shift+Z to get them back.</p>
  </section>
</div>

<style>
  .page {
    display: grid;
    gap: var(--space-5);
    min-width: 0;
  }
  .failure {
    align-items: flex-start;
    justify-content: space-between;
    flex-wrap: wrap;
  }
  .failure ul {
    margin: 0;
    padding-left: 1.2em;
  }
  .apps {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .apps li {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    padding: var(--space-3) var(--space-4);
    border-top: 1px solid var(--line);
  }
  .apps li:first-child {
    border-top: 0;
  }
  .name {
    font-weight: 600;
    min-width: 0;
    overflow-wrap: anywhere;
  }
  .apps .name {
    flex: 1;
  }
  .player {
    display: grid;
    gap: var(--space-3);
    min-width: 0;
  }
  .player[hidden] {
    display: none;
  }
  .bar {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    flex-wrap: wrap;
  }
  .spacer {
    flex: 1;
  }
  .surface {
    position: relative;
    aspect-ratio: 16 / 9;
    background: #000;
    border-radius: var(--radius);
    overflow: hidden;
    /* Touch drags are the host's pointer, not the page's scroll or zoom. */
    touch-action: none;
  }
  .surface:focus-visible {
    outline: 2px solid var(--focus);
    outline-offset: 2px;
  }
  .surface:fullscreen {
    border-radius: 0;
  }
  video {
    display: block;
    width: 100%;
    height: 100%;
    object-fit: contain;
    pointer-events: none;
  }
  .hint {
    font-size: var(--text-sm);
  }
</style>
