<script lang="ts">
  // The play page (docs/webrtc.md, "Play in a TV browser"): its own small build under /play/ on
  // the guest port, for TV, console and car browsers. It pairs once with a PIN the PC's console
  // confirms, lists the PC's games for a remote, the arrow keys or a pad, and plays one full
  // screen. Holding Back, Escape or the pad's Select for a second opens the play dock, and so does
  // the handle at the top edge. Rubylight 7.0: the pairing card, the library, the dock, the stats
  // capsule and the glyph bar are components of their own beside this file.
  import { onDestroy, onMount, tick } from 'svelte';
  import BrandMark from '../components/BrandMark.svelte';
  import PlayDock from './PlayDock.svelte';
  import PlayErrorCard from './PlayErrorCard.svelte';
  import PlayGlyphBar from './PlayGlyphBar.svelte';
  import PlayLibrary from './PlayLibrary.svelte';
  import PlayPairCard from './PlayPairCard.svelte';
  import PlayStatsCapsule from './PlayStatsCapsule.svelte';
  import { activePad, firstSource, hints, keySource, pointerSource, type HintContext, type InputSource } from './play-input-source';
  import { CODECS, codecName, defaultChoice, hardwareCodecs, optionFor, receivableCodecs, streamOptions, type AppEntry, type Choice, type StreamOption } from '../lib/webrtc/client';
  import { buttonChanges, cropPadding, keyMessage, positionMessage, shownSize, Wheel } from '../lib/webrtc/input';
  import { deviceName, forgetCredentials, Hold, keyAction, loadCredentials, nearest, newPin, PadNavigator, PlayStream, saveCredentials, type Action, type Move } from '../lib/webrtc/play';
  import { RefusedError, SignallingError } from '../lib/webrtc/signalling';
  import { readStats, type Look, type StreamStats } from '../lib/webrtc/stats';

  type Phase = 'connecting' | 'pair' | 'library' | 'starting' | 'stream' | 'error';
  const CHOICE_KEY = 'rubylight.play.picture';
  /** Whether the stats capsule shows: this browser's choice, never a host setting. */
  const STATS_KEY = 'rubylight.play.stats';
  const HINT_MS = 5000;
  const STATS_MS = 1000;

  let phase = $state<Phase>('connecting');
  let message = $state('');
  let notice = $state('');
  let pin = $state('');
  let pairError = $state('');
  let loading = $state(false);
  let apps = $state.raw<AppEntry[]>([]);
  let covers = $state<Record<string, string>>({});
  let options = $state.raw<StreamOption[]>([]);
  let choice = $state.raw<Choice | null>(null);
  let status = $state('');
  let hint = $state(false);
  let menu = $state(false);
  let muted = $state(false);
  let host = $state('');
  /** The game that plays, for the dock. */
  let playing = $state('');
  let fullscreen = $state(false);
  let canFullscreen = $state(false);
  let statsOn = $state(savedStats());
  let stats = $state.raw<StreamStats | null>(null);
  const agent = typeof navigator === 'undefined' ? '' : navigator.userAgent;
  /** What the page was last used with: the glyph bar draws that input's buttons. */
  let source = $state<InputSource>(firstSource(agent, false));
  let video = $state<HTMLVideoElement>();
  let surface = $state<HTMLDivElement>();
  let screen = $state<HTMLElement>();
  let page = $state<HTMLElement>();
  let menuBox = $state<HTMLElement>();
  let stream: PlayStream | null = null;
  let cancelPairing: (() => void) | null = null;
  /** The pairing or start in progress: a late answer to an earlier one is ignored. */
  let attempt: object | null = null;
  const name = deviceName(agent);
  const pad = new PadNavigator();
  const padHold = new Hold();
  const keyHold = new Hold();
  let backKey = false;
  let context = '';
  let frame = 0;
  let hintTimer: ReturnType<typeof setTimeout> | undefined;
  let gone = false;
  let buttons = 0;
  let wheelInput = new Wheel();

  const live = $derived(phase === 'stream' || phase === 'starting');
  const codecs = $derived(CODECS.filter((c) => options.some((o) => o.codec === c.id)));
  const picture = $derived(choice ? `${codecName(choice.codec)} · ${choice.height}p${choice.fps}` : '');
  /** What a press does on the screen that shows, for the glyph bar; null where nothing can be pressed. */
  const hintContext = $derived<HintContext | null>(
    phase === 'library' ? (apps.length ? 'library' : null) : phase === 'error' ? 'error' : phase === 'pair' && pairError ? 'pair-failed' : null,
  );
  const footHints = $derived(hintContext !== null && hints(source, hintContext).length > 0);
  /** Touch and mouse get no hint, so the handle shows itself for a moment instead. */
  const pointing = $derived(source === 'touch' || source === 'mouse');

  const errorText = (error: unknown) => (error instanceof Error ? error.message : String(error));

  function savedChoice(): Choice | null {
    try {
      return JSON.parse(localStorage.getItem(CHOICE_KEY) ?? 'null') as Choice | null;
    } catch {
      return null;
    }
  }

  function savedStats(): boolean {
    try {
      return localStorage.getItem(STATS_KEY) === '1';
    } catch {
      return false;
    }
  }

  async function connect() {
    phase = 'connecting';
    message = '';
    const saved = loadCredentials();
    if (!saved) return pair();
    try {
      await ready(await PlayStream.signIn(saved));
    } catch (error) {
      if (error instanceof SignallingError && error.code === 'authentication-failed') {
        forgetCredentials();
        notice = 'This device is no longer paired with the PC.';
        return pair();
      }
      if (error instanceof SignallingError && error.code === 'client-disabled') {
        return failed(new Error('This device is turned off on the PC’s Devices page.'));
      }
      return failed(error);
    }
  }

  async function pair() {
    cancelPairing?.();
    const current = (attempt = {});
    phase = 'pair';
    pairError = '';
    pin = newPin();
    try {
      const pairing = await PlayStream.pairing(pin, name);
      cancelPairing = pairing.cancel;
      if (attempt !== current) return pairing.cancel();
      const { stream: paired, credentials } = await pairing.result;
      cancelPairing = null;
      try {
        saveCredentials(credentials);
      } catch (error) {
        notice = errorText(error);
      }
      await ready(paired);
    } catch (error) {
      if (gone || attempt !== current) return;
      cancelPairing = null;
      const code = error instanceof SignallingError ? error.code : '';
      if (error instanceof RefusedError) return failed(error);
      pairError =
        code === 'incorrect-pin'
          ? 'The PIN entered on the PC did not match.'
          : code === 'pairing-expired'
            ? 'The PIN ran out before the PC confirmed it.'
            : code === 'pairing-cancelled'
              ? 'The PC turned the request down.'
              : errorText(error);
      console.error('Pairing failed:', error);
      await tick();
      focusFirst();
    }
  }

  /** Says why the page cannot play; a refused socket asks the page's own address why. */
  async function failed(error: unknown) {
    if (gone) return;
    console.error('Rubylight:', error);
    let text = errorText(error);
    if (error instanceof RefusedError) {
      const answer = await fetch(location.pathname, { method: 'HEAD', cache: 'no-store' }).then(
        (response) => response.status,
        () => 0,
      );
      text =
        answer === 403
          ? 'This PC lets browsers play on its local network and Tailscale only.'
          : answer === 404
            ? 'Playing in the browser is off on this PC: turn on the WebRTC server in the Rubylight console.'
            : answer === 0
              ? 'The PC does not answer. Check that Rubylight is running.'
              : 'The PC refused this page’s connection.';
    }
    attempt = null;
    cancelPairing?.();
    cancelPairing = null;
    stream?.close();
    stream = null;
    message = text;
    phase = 'error';
    await tick();
    focusFirst();
  }

  async function ready(opened: PlayStream) {
    stream = opened;
    host = opened.gatewayName;
    opened.onended = (reason, closed) => ended(opened, reason, closed);
    const receivable = receivableCodecs(RTCRtpReceiver.getCapabilities?.('video'));
    options = streamOptions(opened.capabilities, receivable);
    choice = defaultChoice(options, await hardwareCodecs(receivable), savedChoice());
    if (!choice) return failed(new Error('This browser cannot play any picture the PC sends.'));
    await library();
  }

  /** The library, with the running flags fresh, and the covers not loaded yet. */
  async function library() {
    const from = stream;
    if (!from) return;
    phase = 'library';
    loading = true;
    try {
      apps = await from.apps();
    } catch (error) {
      return failed(error);
    } finally {
      loading = false;
    }
    await tick();
    focusFirst();
    for (const app of apps) {
      if (stream !== from || phase !== 'library') return;
      if (!app.artworkAvailable || covers[app.id]) continue;
      try {
        const cover = await from.artwork(app.id);
        if (cover) covers = { ...covers, [app.id]: cover };
      } catch (error) {
        console.error(`The cover of ${app.title} did not load:`, error);
      }
    }
  }

  function nextPicture() {
    if (!choice || codecs.length < 2) return;
    const index = codecs.findIndex((c) => c.id === choice!.codec);
    const codec = codecs[(index + 1) % codecs.length]!.id;
    const wanted = { ...choice, codec };
    choice = optionFor(options, wanted) ? wanted : defaultChoice(options, [codec]);
    try {
      localStorage.setItem(CHOICE_KEY, JSON.stringify(choice));
    } catch {
      // Not remembered; the default comes back next time.
    }
  }

  async function start(app: AppEntry) {
    if (!stream || !choice || !video) return;
    const current = (attempt = {});
    notice = '';
    status = 'Starting…';
    menu = false;
    playing = app.title;
    stats = null;
    phase = 'starting';
    // Sound needs a key press or click on the page first; a pad press is none.
    const activation = (navigator as Navigator & { userActivation?: { hasBeenActive: boolean } }).userActivation;
    muted = activation ? !activation.hasBeenActive : false;
    video.muted = muted;
    // Full screen needs that too; without it the picture still fills the page.
    enterFullscreen();
    try {
      await stream.start(app.id, choice, video, (text) => (status = text || 'Starting…'));
    } catch (error) {
      if (gone || attempt !== current || phase !== 'starting') return;
      console.error('The stream did not start:', error);
      leaveFullscreen();
      if (stream?.closed) return failed(error);
      notice = `${app.title} did not start: ${errorText(error)}`;
      return library();
    }
    if (attempt !== current) return;
    phase = 'stream';
    status = '';
    buttons = 0;
    wheelInput = new Wheel();
    hint = true;
    clearTimeout(hintTimer);
    hintTimer = setTimeout(() => (hint = false), HINT_MS);
    surface?.focus();
  }

  function leaveFullscreen() {
    if (document.fullscreenElement) void document.exitFullscreen().catch(() => {});
  }

  /** Full screen, with Escape kept for the game where the browser can (it opens the dock when held). */
  function enterFullscreen() {
    document.documentElement
      .requestFullscreen?.()
      .then(() => (navigator as Navigator & { keyboard?: { lock?: (keys: string[]) => Promise<void> } }).keyboard?.lock?.(['Escape']))
      .catch(() => {});
  }

  const toggleFullscreen = () => (document.fullscreenElement ? leaveFullscreen() : enterFullscreen());

  function toggleMute() {
    if (!video) return;
    video.muted = muted = !muted;
  }

  function toggleStats() {
    statsOn = !statsOn;
    try {
      localStorage.setItem(STATS_KEY, statsOn ? '1' : '0');
    } catch {
      // Not remembered; the capsule is off again next time.
    }
  }

  // The capsule's numbers: the browser is asked a second after its last answer, and only while the capsule shows.
  $effect(() => {
    if (phase !== 'stream' || !statsOn) return;
    let last: Look | null = null;
    let over = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const look = async () => {
      const report = await stream?.stats().catch(() => null);
      if (over) return;
      if (report) {
        const read = readStats(report, last, performance.now());
        last = read.look;
        stats = read.stats;
      }
      timer = setTimeout(look, STATS_MS);
    };
    void look();
    return () => ((over = true), clearTimeout(timer));
  });

  function ended(from: PlayStream, reason: string, closed: boolean) {
    if (stream !== from || gone) return;
    attempt = null;
    menu = false;
    leaveFullscreen();
    if (closed && /another tab/i.test(from.closeReason)) return failed(new Error(reason));
    notice = reason;
    // A closed socket signs in again: a device unpaired on the PC pairs anew.
    if (closed) void connect();
    else void library();
  }

  async function openMenu() {
    stream?.releaseAll();
    buttons = 0;
    // The focus moves to the menu, so the picture never sees this key let go.
    backKey = false;
    menu = true;
    await tick();
    menuBox?.querySelector<HTMLElement>('[data-nav]')?.focus();
  }

  function closeMenu() {
    menu = false;
    surface?.focus();
  }

  function disconnect() {
    attempt = null;
    menu = false;
    stream?.disconnect();
    leaveFullscreen();
    void library();
  }

  async function quit() {
    attempt = null;
    menu = false;
    status = 'Quitting…';
    leaveFullscreen();
    try {
      await stream?.quit();
    } catch (error) {
      notice = errorText(error);
    }
    status = '';
    void library();
  }

  function focusFirst() {
    (menuBox ?? screen)?.querySelector<HTMLElement>('[data-nav]')?.focus();
  }

  /** The focus moves over the menu while it is open, else over the page with its header. */
  function move(direction: Move) {
    const items = [...((menu ? menuBox : page)?.querySelectorAll<HTMLElement>('[data-nav]') ?? [])];
    const index = items.indexOf(document.activeElement as HTMLElement);
    if (index < 0) return items[0]?.focus();
    const boxes = items.map((item) => item.getBoundingClientRect());
    const next = items[nearest(boxes[index]!, boxes, direction)];
    next?.focus();
    next?.scrollIntoView({ block: 'nearest', inline: 'nearest' });
  }

  function act(action: Action) {
    if (action === 'menu') return;
    if (action === 'back') {
      if (menu) closeMenu();
      return;
    }
    if (action === 'select') return (document.activeElement as HTMLElement | null)?.click();
    move(action);
  }

  /** Pads: to the host while a game plays, else they move the focus. */
  function loop(now: number) {
    const pads = navigator.getGamepads?.() ?? [];
    const family = activePad(pads);
    if (family && family !== source) source = family;
    const here = `${phase}${menu}`;
    if (here !== context) {
      // What was held when the screen changed acts only once let go.
      context = here;
      pad.poll(pads, now);
    } else if (live && stream && !menu) {
      if (phase === 'stream') stream.pollPads(pads, now);
      const select = pads.some((p) => p?.buttons[8]?.pressed);
      if (padHold.update(select, now) || keyHold.update(backKey, now)) void openMenu();
    } else {
      if (live) stream?.pollPads(pads, now, true);
      for (const action of pad.poll(pads, now)) act(action);
    }
    frame = requestAnimationFrame(loop);
  }

  $effect(() => {
    if (video && choice) return cropPadding(video, { width: choice.width, height: choice.height });
  });

  function windowKey(event: KeyboardEvent) {
    const from = keySource(event, agent);
    if (from !== source) source = from;
    if (live && !menu) return;
    const action = keyAction(event);
    if (!action) return;
    event.preventDefault();
    // Holding Back opened the menu; its repeats must not close it again.
    if (action === 'back' && event.repeat) return;
    act(action);
  }

  // Keys and the mouse go to the PC while the picture has the focus.
  function key(event: KeyboardEvent, down: boolean) {
    if (phase !== 'stream' || menu) return;
    if (keyAction(event) === 'back' && !event.repeat) backKey = down;
    event.preventDefault();
    unmute();
    stream?.send(keyMessage(event.code, down));
  }

  function unmute() {
    if (!muted || !video) return;
    video.muted = muted = false;
  }

  function pointer(event: PointerEvent) {
    if (phase !== 'stream' || menu || !video) return;
    if (event.type === 'pointerdown') {
      event.preventDefault();
      unmute();
      surface?.focus();
      surface?.setPointerCapture(event.pointerId);
    }
    const size = { width: video.videoWidth, height: video.videoHeight };
    const shown = choice ? shownSize(size, choice) : size;
    stream?.send(positionMessage(event.clientX, event.clientY, video.getBoundingClientRect(), size, shown));
    pressed(event.buttons);
  }

  function pressed(mask: number) {
    for (const change of buttonChanges(buttons, mask)) stream?.send(change);
    buttons = mask;
  }

  function wheel(event: WheelEvent) {
    if (phase !== 'stream' || menu) return;
    event.preventDefault();
    stream?.send(wheelInput.message(event.deltaX, event.deltaY, event.deltaMode));
  }

  function blurred() {
    stream?.releaseAll();
    buttons = 0;
    backKey = false;
  }

  function pointed(event: PointerEvent) {
    const from = pointerSource(event.pointerType);
    if (from !== source) source = from;
  }

  onMount(() => {
    // Edge on Xbox hands the pad to the page only when asked; it moves a pointer otherwise.
    const xbox = navigator as Navigator & { gamepadInputEmulation?: string };
    if ('gamepadInputEmulation' in xbox) xbox.gamepadInputEmulation = 'gamepad';
    canFullscreen = typeof document.documentElement.requestFullscreen === 'function';
    source = firstSource(agent, matchMedia('(pointer: coarse)').matches);
    frame = requestAnimationFrame(loop);
    void connect();
  });

  onDestroy(() => {
    gone = true;
    if (frame) cancelAnimationFrame(frame);
    clearTimeout(hintTimer);
    cancelPairing?.();
    stream?.close();
  });
</script>

<svelte:window onkeydown={windowKey} onpointerdown={pointed} onpointermove={pointed} onpagehide={() => stream?.close()} />
<svelte:document onfullscreenchange={() => (fullscreen = !!document.fullscreenElement)} />

<main class="media-plane" class:live class:hinted={footHints} bind:this={page}>
  {#if !live}
    <header class="top">
      <BrandMark size={40} />
      <span class="host">{host || 'Rubylight'}</span>
      {#if phase === 'library' && choice}
        {#if codecs.length > 1}
          <button class="chip" data-nav onclick={nextPicture} aria-label="Picture: {codecName(choice.codec)}, {choice.height}p, {choice.fps} fps. Press for the next codec.">
            <span class="chip-label" aria-hidden="true">Picture</span>{picture}
          </button>
        {:else}
          <span class="chip"><span class="chip-label">Picture</span>{picture}</span>
        {/if}
      {/if}
    </header>
    {#if notice && (phase === 'library' || phase === 'pair')}<p class="notice" role="alert">{notice}</p>{/if}
  {/if}

  <section class="screen" class:centred={phase !== 'library'} bind:this={screen} hidden={live}>
    {#if phase === 'connecting'}
      <p class="connecting" role="status"><span class="pulse" aria-hidden="true"></span>Connecting…</p>
    {:else if phase === 'pair'}
      <PlayPairCard {pin} {name} error={pairError} onretry={pair} />
    {:else if phase === 'library'}
      <PlayLibrary {apps} {covers} {loading} onstart={start} />
    {:else if phase === 'error'}
      <PlayErrorCard {message} onretry={connect} />
    {/if}
  </section>
  {#if !live && footHints && hintContext}
    <footer><PlayGlyphBar {source} context={hintContext} /></footer>
  {/if}

  <section class="player" hidden={!live} aria-label="Game">
    <!-- The picture takes every key and pointer event while it has the focus. -->
    <!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
    <div
      class="surface"
      bind:this={surface}
      tabindex="0"
      role="application"
      aria-label="Game picture"
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
      <video id="play-video" bind:this={video} autoplay playsinline></video>
    </div>
    {#if status}<p class="status" role="status"><span class="pulse" aria-hidden="true"></span>{status}</p>{/if}
    {#if phase === 'stream' && !menu}
      <!-- Nothing but the game shows once the first seconds have passed. -->
      {#if muted}<p class="sound" role="status">Sound is off. Press a key or click to hear it.</p>{/if}
      {#if hint}<PlayGlyphBar {source} context="stream" floating />{/if}
      {#if statsOn}<PlayStatsCapsule {stats} codec={picture} />{/if}
      <!-- The pointer's way to the dock; a key or a pad holds Back or Select instead. -->
      <button class="handle" class:peek={hint && pointing} tabindex="-1" aria-label="Stream menu" onclick={openMenu}>
        <span aria-hidden="true"></span>
      </button>
    {/if}
    {#if menu}
      <PlayDock
        bind:box={menuBox}
        title={playing}
        {picture}
        {muted}
        stats={statsOn}
        {fullscreen}
        {canFullscreen}
        {source}
        onresume={closeMenu}
        onfullscreen={toggleFullscreen}
        onmute={toggleMute}
        onstats={toggleStats}
        ondisconnect={disconnect}
        onquit={quit}
      />
    {/if}
  </section>
</main>

<style>
  main {
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    gap: 1.5em;
    align-content: start;
    min-height: 100vh;
    box-sizing: border-box;
    /* TVs crop the edges of the picture. */
    padding: 5vh 5vw;
    background-image: var(--media-glow);
    background-repeat: no-repeat;
    font-size: clamp(18px, 1.5vw, 32px);
  }
  main.hinted {
    padding-bottom: calc(5vh + 4em);
  }
  main.live {
    padding: 0;
  }
  .top {
    display: flex;
    align-items: center;
    gap: 0.75em;
    font-weight: var(--weight-semibold);
  }
  .host {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    font-family: var(--font-display);
    font-size: 1.15em;
    letter-spacing: var(--tracking-tight);
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  [hidden] {
    display: none !important;
  }
  button {
    font: inherit;
    color: inherit;
    cursor: pointer;
  }
  .chip {
    display: inline-flex;
    align-items: center;
    gap: 0.6em;
    min-height: max(44px, 2.4em);
    box-sizing: border-box;
    padding: 0.3em 1em;
    border: 2px solid var(--media-line);
    border-radius: var(--radius-pill);
    background: var(--media-panel);
    font-size: 0.8em;
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }
  .chip-label {
    color: var(--media-idle);
    font-size: 0.8em;
    letter-spacing: var(--tracking-label);
    text-transform: uppercase;
  }
  .chip:focus {
    outline: 4px solid var(--media-focus);
    outline-offset: 4px;
  }
  .screen.centred {
    display: grid;
    align-content: center;
    min-height: 70vh;
  }
  p {
    margin: 0;
  }
  .notice {
    justify-self: start;
    padding: 0.6em 1em;
    border: 1px solid color-mix(in srgb, var(--media-warn) 36%, var(--media-panel));
    border-radius: var(--radius);
    background: color-mix(in srgb, var(--media-warn) 14%, var(--media-panel));
    color: var(--media-ink);
    font-size: 0.85em;
  }
  .connecting,
  .status {
    display: flex;
    align-items: center;
    gap: 0.7em;
    color: var(--media-ink-2);
  }
  .connecting {
    justify-self: center;
  }
  .pulse {
    flex: none;
    width: 0.6em;
    height: 0.6em;
    border-radius: var(--radius-pill);
    background: var(--media-accent-text);
    animation: breathe var(--pulse-duration) var(--ease-standard) infinite alternate;
  }
  @keyframes breathe {
    to {
      opacity: 0.3;
    }
  }
  footer {
    position: fixed;
    right: 0;
    bottom: 0;
    left: 0;
    z-index: var(--z-bar);
    padding: 1.2em 5vw max(3vh, env(safe-area-inset-bottom));
    border-top: 1px solid var(--media-line);
    background: var(--media-bg);
  }
  .player {
    position: fixed;
    inset: 0;
    background: var(--media-black);
  }
  .surface {
    position: absolute;
    inset: 0;
    overflow: hidden;
    outline: none;
    touch-action: none;
  }
  video {
    width: 100%;
    height: 100%;
    object-fit: contain;
    pointer-events: none;
  }
  /* Over the picture: solid decks, never a blur. */
  .status,
  .sound {
    position: absolute;
    left: 50%;
    z-index: var(--z-raised);
    transform: translateX(-50%);
    max-width: 90vw;
    box-sizing: border-box;
    padding: 0.6em 1.1em;
    border: 1px solid var(--media-line);
    border-radius: var(--radius-pill);
    background: var(--media-panel);
    box-shadow: var(--media-shadow);
    color: var(--media-ink);
    font-size: 0.85em;
    text-align: center;
  }
  .status {
    top: 45%;
  }
  .sound {
    top: max(3vh, env(safe-area-inset-top));
    pointer-events: none;
  }
  /* The handle: a strip at the top edge that shows a grip under the pointer. */
  .handle {
    position: absolute;
    top: 0;
    left: 50%;
    z-index: var(--z-raised);
    display: grid;
    place-items: start center;
    width: min(12em, 60vw);
    height: 44px;
    padding: 8px 0 0;
    border: 0;
    background: none;
    opacity: 0;
    transform: translateX(-50%);
    transition: opacity var(--duration-base) var(--ease-standard);
  }
  .handle span {
    width: 4em;
    height: 8px;
    border: 1px solid var(--media-line-strong);
    border-radius: var(--radius-pill);
    background: var(--media-ink-2);
    box-shadow: var(--media-shadow);
  }
  .handle:hover,
  .handle:focus-visible,
  .handle.peek {
    opacity: 1;
  }
  @media (max-width: 600px) {
    .chip-label {
      display: none;
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .pulse {
      animation: none;
    }
    .handle {
      transition: none;
    }
  }
</style>
