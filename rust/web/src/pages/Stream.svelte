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
  import { buttonMessage, keyMessage, positionMessage, wheelMessage } from '../lib/webrtc/input';
  import { RefusedError } from '../lib/webrtc/signalling';

  type Phase = 'checking' | 'off' | 'ready' | 'starting' | 'streaming' | 'failed';

  let phase = $state<Phase>('checking');
  let error = $state('');
  let refused = $state(false);
  let status = $state('');
  let anyNetwork = $state(false);
  let apps = $state.raw<AppEntry[]>([]);
  let title = $state('');
  let video = $state<HTMLVideoElement>();
  let surface = $state<HTMLDivElement>();
  let stream: ConsoleStream | null = null;
  let gone = false;

  const live = $derived(phase === 'starting' || phase === 'streaming');

  const message = (cause: unknown) => (cause instanceof Error ? cause.message : String(cause));

  async function connect() {
    stream?.close();
    stream = null;
    phase = 'checking';
    error = '';
    refused = false;
    try {
      // Also renews the sign-in the socket rides on.
      const config = await api.config.get();
      anyNetwork = String(config.origin_web_ui_allowed ?? '').trim() === 'wan';
      if (!on(config, 'webrtc_server', false)) {
        phase = 'off';
        return;
      }
      const opened = await ConsoleStream.connect(await sessionCsrfToken());
      if (gone) return opened.close();
      stream = opened;
      opened.onended = (reason) => ended(opened, reason);
      apps = await opened.apps();
      phase = 'ready';
    } catch (cause) {
      refused = cause instanceof RefusedError;
      error = message(cause);
      phase = 'failed';
    }
  }

  function ended(from: ConsoleStream, reason: string) {
    if (stream !== from || gone) return;
    error = reason;
    status = '';
    if (document.fullscreenElement) void document.exitFullscreen();
    phase = phase === 'streaming' || phase === 'starting' ? 'ready' : phase;
    void refreshApps();
  }

  async function refreshApps() {
    const current = stream;
    if (!current) return;
    try {
      apps = await current.apps();
    } catch (cause) {
      // The socket is gone: start over from the sign-in.
      if (stream === current) {
        error = message(cause);
        phase = 'failed';
      }
    }
  }

  async function start(app: AppEntry) {
    const current = stream;
    if (!current || !video || phase !== 'ready') return;
    error = '';
    title = app.title;
    phase = 'starting';
    try {
      await current.start(app.id, video, (text) => (status = text));
      if (stream !== current || phase !== 'starting') return;
      phase = 'streaming';
      surface?.focus();
    } catch (cause) {
      // Stopped by the page, or ended by the host: `stop` or `ended` has said so already.
      if (stream !== current || phase !== 'starting') return;
      error = message(cause);
      status = '';
      phase = 'ready';
      void refreshApps();
    }
  }

  function stop(quitApp: boolean) {
    stream?.stop(quitApp);
    status = '';
    if (document.fullscreenElement) void document.exitFullscreen();
    phase = 'ready';
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

  function button(event: PointerEvent, down: boolean) {
    if (phase !== 'streaming') return;
    event.preventDefault();
    if (down) {
      surface?.focus();
      surface?.setPointerCapture(event.pointerId);
      position(event);
    }
    stream?.send(buttonMessage(event.button, down));
  }

  function wheel(event: WheelEvent) {
    if (phase !== 'streaming') return;
    event.preventDefault();
    stream?.send(wheelMessage(event.deltaX, event.deltaY, event.deltaMode));
  }

  onMount(() => void connect());
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
    <p class="muted" role="status">Connecting to the host…</p>
  {:else if phase === 'off'}
    <EmptyState icon="monitor" title="The WebRTC server is off">
      <p>Turn on <strong>WebRTC server</strong> in <a href="/settings/network" use:link>Settings, Network</a>, then restart Rubylight.</p>
    </EmptyState>
  {:else if phase === 'failed'}
    <div class="notice danger failure" role="alert">
      <div class="stack">
        {#if refused}
          <strong>The host refused the stream connection.</strong>
          <span>The browser does not say why. One of these is the cause:</span>
          <ul>
            <li>This tab is not signed in, or it opened the console from an address the host does not list as its own (see <code>csrf_allowed_origins</code>).</li>
            <li>This device is not on the local network or Tailscale.</li>
            <li>The WebRTC server is off, or was turned on without restarting Rubylight.</li>
            <li>Your console device, “Console ({'<'}your user{'>'})”, is disabled on <a href="/devices" use:link>Devices</a>.</li>
          </ul>
        {:else}
          <span>The stream connection failed: {error}</span>
        {/if}
      </div>
      <Button size="sm" icon="refresh" onclick={connect}>Try again</Button>
    </div>
  {:else}
    {#if error}
      <p class="notice danger" role="alert">{error}</p>
    {/if}
    {#if phase === 'ready'}
      <Panel title="Games & apps" description="Pick one to start it on this PC and stream it here." flush>
        {#if apps.length}
          <ul class="apps">
            {#each apps as app (app.id)}
              <li>
                <span class="name">{app.title}</span>
                {#if app.running}<Badge tone="live">Running</Badge>{/if}
                <Button size="sm" icon="play" onclick={() => start(app)}>Stream<span class="visually-hidden"> {app.title}</span></Button>
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
      <Button size="sm" variant="secondary" onclick={() => stop(false)}>Disconnect</Button>
      <Button size="sm" variant="danger" icon="stop" onclick={() => stop(true)}>Quit app</Button>
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
      onpointermove={position}
      onpointerdown={(event) => button(event, true)}
      onpointerup={(event) => button(event, false)}
      onwheel={wheel}
      oncontextmenu={(event) => event.preventDefault()}
      onblur={() => stream?.releaseAll()}
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
