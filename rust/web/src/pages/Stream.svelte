<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import Badge from '../components/Badge.svelte';
  import Button from '../components/Button.svelte';
  import EmptyState from '../components/EmptyState.svelte';
  import Field from '../components/Field.svelte';
  import PageHeader from '../components/PageHeader.svelte';
  import Panel from '../components/Panel.svelte';
  import Select from '../components/Select.svelte';
  import GuestPanel from '../components/stream/GuestPanel.svelte';
  import { api, sessionCsrfToken } from '../lib/api';
  import { link } from '../lib/router.svelte';
  import { on } from '../lib/schema/basics';
  import {
    CODECS,
    ConsoleStream,
    codecName,
    defaultChoice,
    fitChoice,
    hardwareCodecs,
    receivableCodecs,
    streamOptions,
    type AppEntry,
    type Choice,
    type Codec,
    type StreamOption,
  } from '../lib/webrtc/client';
  import { buttonChanges, cropPadding, keyMessage, positionMessage, shownSize, Wheel } from '../lib/webrtc/input';
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
  /** What both this browser and the host can stream, and what the page asks for. */
  let options = $state.raw<StreamOption[]>([]);
  let choice = $state<Choice | null>(null);

  const CHOICE_KEY = 'butterpollo.stream';
  // A browser that blocks site data throws on localStorage: the choice then lasts for this visit.
  function savedChoice(): Choice | null {
    try {
      const value = JSON.parse(localStorage.getItem(CHOICE_KEY) ?? 'null') as Choice | null;
      return value && typeof value.codec === 'string' && [value.width, value.height, value.fps].every(Number.isInteger) ? value : null;
    } catch {
      return null;
    }
  }
  function choose(next: Choice | null) {
    if (!next) return;
    choice = next;
    try {
      localStorage.setItem(CHOICE_KEY, JSON.stringify(next));
    } catch {
      // Not remembered; see savedChoice().
    }
  }

  const codecOptions = $derived(CODECS.filter((c) => options.some((o) => o.codec === c.id)).map((c) => ({ value: c.id, label: c.name })));
  const sizeOptions = $derived(
    options
      .filter((o) => o.codec === choice?.codec)
      .sort((a, b) => a.width * a.height - b.width * b.height)
      .map((o) => ({ value: `${o.width}x${o.height}`, label: `${o.width}×${o.height}` })),
  );
  const rateOptions = $derived(
    (options.find((o) => o.codec === choice?.codec && o.width === choice?.width && o.height === choice?.height)?.frameRates ?? []).map((f) => ({
      value: String(f),
      label: `${f} fps`,
    })),
  );
  const pickCodec = (codec: string) => choice && choose(fitChoice(options, { ...choice, codec: codec as Codec }));
  function pickSize(size: string) {
    const [width, height] = size.split('x').map(Number);
    if (choice) choose(fitChoice(options, { ...choice, width: width!, height: height! }));
  }
  const pickRate = (fps: string) => choice && choose({ ...choice, fps: Number(fps) });

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
      const receivable = receivableCodecs(RTCRtpReceiver.getCapabilities?.('video'));
      const offered = streamOptions(opened.capabilities, receivable);
      if (!offered.length) {
        const names = receivable.map(codecName).join(', ') || 'none';
        throw new Error(`The host offers no format this browser receives over WebRTC (this browser: ${names}).`);
      }
      const preferred = await hardwareCodecs(receivable);
      apps = await opened.apps();
      if (stream !== opened) return;
      options = offered;
      choice = defaultChoice(offered, preferred, savedChoice());
      phase = 'ready';
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
    if (!current || !video || !choice || phase !== 'ready') return;
    error = '';
    title = app.title;
    phase = 'starting';
    buttons = 0;
    wheelInput = new Wheel();
    // Called before the first await: `video.play()` needs the click's user gesture in Safari.
    const started = current.start(app.id, choice, video, (text) => (status = text));
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
    const shown = choice ? shownSize(size, choice) : size;
    stream?.send(positionMessage(event.clientX, event.clientY, video.getBoundingClientRect(), size, shown));
  }

  $effect(() => {
    if (video && choice) return cropPadding(video, { width: choice.width, height: choice.height });
  });

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
  <PageHeader title="Stream" subtitle="Play a game from this PC in this browser tab, with stereo sound, keyboard and mouse.">
  </PageHeader>

  {#if anyNetwork}
    <p class="notice warn">Console access is set to any network, but streaming works on the local network and Tailscale only.</p>
  {/if}

  {#if phase === 'checking'}
    <p class="muted" role="status">Checking the host…</p>
  {:else if phase === 'connecting'}
    <p class="muted" role="status">Connecting to the host…</p>
  {:else if phase === 'off'}
    <Panel>
      <EmptyState icon="play" title="The WebRTC server is off">
        <p>Streaming in this tab needs it. Turn on <strong>WebRTC server</strong> in Network &amp; access, save, then restart Rubylight when the console asks.</p>
        <Button variant="primary" href="/settings/network?setting=webrtc_server" icon="settings">Open the WebRTC setting</Button>
      </EmptyState>
    </Panel>
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
      <Panel title="Games" description="Connect to list them. One tab or browser streams at a time: connecting here takes over from another.">
        <div><Button icon="play" onclick={connect}>Connect</Button></div>
      </Panel>
    {:else if phase === 'ready'}
      {#if choice}
        <Panel title="Picture">
          <div class="picture">
            <Field label="Codec" id="stream-codec">
              <Select id="stream-codec" value={choice.codec} options={codecOptions} onchange={pickCodec} />
            </Field>
            <Field label="Resolution" id="stream-size">
              <Select id="stream-size" value="{choice.width}x{choice.height}" options={sizeOptions} onchange={pickSize} />
            </Field>
            <Field label="Frame rate" id="stream-fps">
              <Select id="stream-fps" value={String(choice.fps)} options={rateOptions} onchange={pickRate} />
            </Field>
          </div>
        </Panel>
      {/if}
      <Panel title="Games" description="Pick one to start it on this PC and stream it here." flush>
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
            <p>Add one in <a href="/library" use:link>Games</a>.</p>
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

  {#if phase !== 'checking' && phase !== 'off' && phase !== 'failed'}
    <GuestPanel />
  {/if}
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
  .picture {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(10rem, 1fr));
    gap: var(--space-4);
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
    font-weight: var(--weight-semibold);
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
    outline: var(--focus-width) solid var(--focus);
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
