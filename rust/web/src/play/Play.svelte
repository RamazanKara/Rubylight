<script lang="ts">
  // The play page (docs/webrtc.md, "Play in a TV browser"): its own small build under /play/ on
  // the guest port, for TV, console and car browsers. It pairs once with a PIN the PC's console
  // confirms, lists the PC's games for a remote, the arrow keys or a pad, and plays one full
  // screen. Holding Back or Select for a second opens Disconnect and Quit game.
  import { onDestroy, onMount, tick } from 'svelte';
  import BrandMark from '../components/BrandMark.svelte';
  import { CODECS, codecName, defaultChoice, hardwareCodecs, optionFor, receivableCodecs, streamOptions, type AppEntry, type Choice, type StreamOption } from '../lib/webrtc/client';
  import { buttonChanges, cropPadding, keyMessage, positionMessage, shownSize, Wheel } from '../lib/webrtc/input';
  import { deviceName, forgetCredentials, Hold, keyAction, loadCredentials, nearest, newPin, PadNavigator, PlayStream, saveCredentials, type Action, type Move } from '../lib/webrtc/play';
  import { RefusedError, SignallingError } from '../lib/webrtc/signalling';

  type Phase = 'connecting' | 'pair' | 'library' | 'starting' | 'stream' | 'error';
  const CHOICE_KEY = 'rubylight.play.picture';
  const HINT_MS = 5000;

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
  let video = $state<HTMLVideoElement>();
  let surface = $state<HTMLDivElement>();
  let screen = $state<HTMLElement>();
  let page = $state<HTMLElement>();
  let menuBox = $state<HTMLElement>();
  let stream: PlayStream | null = null;
  let cancelPairing: (() => void) | null = null;
  /** The pairing or start in progress: a late answer to an earlier one is ignored. */
  let attempt: object | null = null;
  const name = deviceName(typeof navigator === 'undefined' ? '' : navigator.userAgent);
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

  const errorText = (error: unknown) => (error instanceof Error ? error.message : String(error));

  function savedChoice(): Choice | null {
    try {
      return JSON.parse(localStorage.getItem(CHOICE_KEY) ?? 'null') as Choice | null;
    } catch {
      return null;
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
    phase = 'starting';
    // Sound needs a key press or click on the page first; a pad press is none.
    const activation = (navigator as Navigator & { userActivation?: { hasBeenActive: boolean } }).userActivation;
    muted = activation ? !activation.hasBeenActive : false;
    video.muted = muted;
    // Full screen needs that too; without it the picture still fills the page.
    document.documentElement
      .requestFullscreen?.()
      .then(() => (navigator as Navigator & { keyboard?: { lock?: (keys: string[]) => Promise<void> } }).keyboard?.lock?.(['Escape']))
      .catch(() => {});
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

  onMount(() => {
    // Edge on Xbox hands the pad to the page only when asked; it moves a pointer otherwise.
    const xbox = navigator as Navigator & { gamepadInputEmulation?: string };
    if ('gamepadInputEmulation' in xbox) xbox.gamepadInputEmulation = 'gamepad';
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

<svelte:window onkeydown={windowKey} onpagehide={() => stream?.close()} />

<main class:live bind:this={page}>
  {#if !live}
    <header>
      <BrandMark size={44} />
      <span class="host">{host || 'Rubylight'}</span>
      {#if phase === 'library' && choice}
        <button class="chip" data-nav onclick={nextPicture} aria-label="Picture: {codecName(choice.codec)}, {choice.height}p, {choice.fps} fps">
          {codecName(choice.codec)} · {choice.height}p{choice.fps}
        </button>
      {/if}
    </header>
    {#if notice && (phase === 'library' || phase === 'pair')}<p class="notice" role="alert">{notice}</p>{/if}
  {/if}

  <section class="screen" bind:this={screen} hidden={live}>
    {#if phase === 'connecting'}
      <p class="muted" role="status">Connecting…</p>
    {:else if phase === 'pair'}
      <div class="card">
        <h1>Pair with your PC</h1>
        <p class="pin" aria-label="PIN {pin.split('').join(' ')}">{pin}</p>
        <p>Enter this PIN in the Rubylight console under Devices.</p>
        <p class="muted">This device: <strong>{name}</strong></p>
        {#if pairError}
          <p class="error" role="alert">{pairError}</p>
          <div><button class="action" data-nav onclick={pair}>New PIN</button></div>
        {:else}
          <p class="muted" role="status">Waiting for the PC…</p>
        {/if}
      </div>
    {:else if phase === 'library'}
      {#if loading}
        <p class="muted" role="status">Loading games…</p>
      {:else if apps.length === 0}
        <p class="muted">No games on this PC yet. Add them in the Rubylight console.</p>
      {/if}
      <ul class="grid">
        {#each apps as app (app.id)}
          <li>
            <button class="tile" data-nav onclick={() => start(app)}>
              <span class="cover">
                {#if covers[app.id]}<img src={covers[app.id]} alt="" />{:else}<span class="initial" aria-hidden="true">{app.title.slice(0, 1)}</span>{/if}
                {#if app.running}<span class="badge">Running</span>{/if}
              </span>
              <span class="title">{app.title}</span>
            </button>
          </li>
        {/each}
      </ul>
    {:else if phase === 'error'}
      <div class="card">
        <h1>Can’t play right now</h1>
        <p class="error" role="alert">{message}</p>
        <div><button class="action" data-nav onclick={connect}>Try again</button></div>
      </div>
    {/if}
  </section>

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
    {#if status}<p class="status" role="status">{status}</p>{/if}
    {#if phase === 'stream' && !menu && (hint || muted)}
      <p class="hint">{muted ? 'Sound is off: press a key or click to hear it. ' : ''}Hold Back or Select for the menu.</p>
    {/if}
    {#if menu}
      <div class="menu" bind:this={menuBox} role="dialog" aria-label="Stream menu">
        <button class="action" data-nav onclick={closeMenu}>Back to game</button>
        <button class="action" data-nav onclick={disconnect}>Disconnect</button>
        <button class="action" data-nav onclick={quit}>Quit game</button>
      </div>
    {/if}
  </section>
</main>

<style>
  main {
    display: grid;
    gap: 1.5em;
    align-content: start;
    min-height: 100vh;
    box-sizing: border-box;
    /* TVs crop the edges of the picture. */
    padding: 5vh 5vw;
    font-size: clamp(18px, 1.5vw, 32px);
  }
  main.live {
    padding: 0;
  }
  header {
    display: flex;
    align-items: center;
    gap: 0.75em;
    font-weight: var(--weight-semibold);
  }
  .host {
    flex: 1;
    font-size: 1.25em;
  }
  [hidden] {
    display: none !important;
  }
  button {
    font: inherit;
    color: inherit;
    cursor: pointer;
  }
  .chip,
  .action {
    padding: 0.6em 1.2em;
    border: 2px solid var(--line);
    border-radius: 999px;
    background: var(--panel);
  }
  .action {
    border-radius: var(--radius);
    min-width: 10em;
  }
  .chip:focus,
  .action:focus,
  .tile:focus {
    outline: 4px solid var(--focus);
    outline-offset: 4px;
    border-color: var(--focus);
  }
  .card {
    display: grid;
    gap: 1em;
    justify-self: center;
    width: min(36em, 100%);
    padding: 1.5em;
    border: 1px solid var(--line);
    border-radius: var(--radius);
    background: var(--panel);
  }
  h1 {
    margin: 0;
    font-size: 1.5em;
  }
  p {
    margin: 0;
  }
  .pin {
    font-size: 4em;
    font-weight: var(--weight-semibold);
    letter-spacing: 0.3em;
    font-variant-numeric: tabular-nums;
  }
  .error,
  .notice {
    color: var(--danger, #ff8a8a);
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(10em, 1fr));
    gap: 1.5em;
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .tile {
    display: grid;
    gap: 0.5em;
    width: 100%;
    padding: 0.4em;
    border: 2px solid transparent;
    border-radius: var(--radius);
    background: none;
    text-align: left;
    transition: transform 0.12s;
  }
  .tile:focus {
    transform: scale(1.05);
  }
  .cover {
    position: relative;
    display: grid;
    place-items: center;
    aspect-ratio: 3 / 4;
    overflow: hidden;
    border-radius: var(--radius);
    background: var(--panel);
  }
  .cover img {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }
  .initial {
    font-size: 3em;
    color: var(--muted);
  }
  .badge {
    position: absolute;
    top: 0.5em;
    left: 0.5em;
    padding: 0.2em 0.6em;
    border-radius: 999px;
    background: var(--accent);
    color: #fff;
    font-size: 0.8em;
  }
  .title {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .player {
    position: fixed;
    inset: 0;
    background: #000;
  }
  .surface {
    position: absolute;
    inset: 0;
    outline: none;
    touch-action: none;
  }
  video {
    width: 100%;
    height: 100%;
    object-fit: contain;
    pointer-events: none;
  }
  .status,
  .hint {
    position: absolute;
    left: 50%;
    transform: translateX(-50%);
    padding: 0.5em 1em;
    border-radius: var(--radius);
    background: rgb(0 0 0 / 0.7);
    color: #fff;
  }
  .status {
    top: 45%;
  }
  .hint {
    bottom: 5vh;
  }
  .menu {
    position: absolute;
    inset: 0;
    display: grid;
    gap: 1em;
    place-content: center;
    background: rgb(0 0 0 / 0.75);
  }
</style>
