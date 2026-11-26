<script lang="ts">
  // The guest page (docs/browser-guests.md): its own small build under /guest/, with nothing of
  // the console's. The invite token is in the link's fragment; the page joins the owner's stream
  // with it and a name, plays picture and sound, and sends what the invite allows. Rubylight 7.0:
  // the page is a stage, dark in both themes, with the join card alone on it until the picture
  // fills it; the stream's bar floats over the picture and hides itself.
  import { onDestroy } from 'svelte';
  import Badge from '../components/Badge.svelte';
  import Button from '../components/Button.svelte';
  import GuestJoinCard, { PERMISSION_TEXT } from './GuestJoinCard.svelte';
  import { GuestStream, gamepads, inviteToken, keyboardMouse, type Ending, type Joined, type Permission } from '../lib/webrtc/guest';
  import { buttonChanges, cropPadding, keyMessage, positionMessage, shownSize, Wheel } from '../lib/webrtc/input';

  type Phase = 'name' | 'joining' | 'waiting' | 'streaming' | 'ended';

  const NAME_KEY = 'rubylight.guest.name';
  /** How often a waiting page asks again whether the owner streams. */
  const WAIT_MS = 5000;
  /** How long the stream's bar stays without the pointer or the focus on it. */
  const BAR_MS = 4000;
  /** The strip at the top of the picture where the pointer brings the bar back. */
  const BAR_ZONE = 96;

  const token = inviteToken(location.hash);
  let phase = $state<Phase>('name');
  let name = $state<string | number | null>(savedName());
  let joined = $state.raw<Joined | null>(null);
  let ended = $state.raw<Ending | null>(null);
  let waiting = $state('');
  /** The browser did not start playback by itself: a click on Play does. */
  let paused = $state(false);
  let video = $state<HTMLVideoElement>();
  let surface = $state<HTMLDivElement>();
  let player = $state<HTMLElement>();
  /** What the invite allowed when the host last offered the stream, for the card's badge. */
  let permission = $state<Permission | null>(null);
  let muted = $state(false);
  let fullscreen = $state(false);
  let barShown = $state(true);
  /** The pointer or the focus is on the bar: it stays. */
  let barHeld = false;
  let barTimer: ReturnType<typeof setTimeout> | undefined;
  let stream: GuestStream | null = null;
  let retry: ReturnType<typeof setTimeout> | undefined;
  let frame = 0;
  let gone = false;
  /** The mouse buttons the host holds, as a `MouseEvent.buttons` mask. */
  let buttons = 0;
  let wheelInput = new Wheel();

  const typing = $derived(joined !== null && keyboardMouse(joined.permission));

  // A browser that blocks site data throws on localStorage: the name then lasts for this visit.
  function savedName(): string {
    try {
      return localStorage.getItem(NAME_KEY) ?? '';
    } catch {
      return '';
    }
  }
  function rememberName() {
    try {
      localStorage.setItem(NAME_KEY, String(name ?? '').trim());
    } catch {
      // Not remembered; see savedName().
    }
  }

  function play(media: MediaStream) {
    paused = false;
    video?.play().catch(() => {
      if (!gone && video?.srcObject === media) paused = true;
    });
  }

  /** Joins the owner's stream. From the click, up to `play`, so Safari plays the sound. */
  function join() {
    if (!token || !video) return;
    clearTimeout(retry);
    rememberName();
    ended = null;
    if (phase !== 'waiting') phase = 'joining';
    const media = new MediaStream();
    video.srcObject = media;
    play(media);
    const current = GuestStream.join(token, String(name ?? '').trim(), media, (live) => streaming(live));
    current.onended = (why) => over(current, why);
    stream = current;
  }

  function streaming(live: GuestStream) {
    if (stream !== live || gone) return;
    joined = live.joined;
    permission = joined?.permission ?? permission;
    phase = 'streaming';
    wake();
    buttons = 0;
    wheelInput = new Wheel();
    if (joined && gamepads(joined.permission)) {
      const tick = (now: number) => {
        stream?.pollPads(navigator.getGamepads?.() ?? [], now);
        frame = requestAnimationFrame(tick);
      };
      frame = requestAnimationFrame(tick);
    }
    surface?.focus();
  }

  function stopped() {
    cancelAnimationFrame(frame);
    frame = 0;
    stream = null;
    joined = null;
    paused = false;
    if (document.fullscreenElement) void document.exitFullscreen();
    if (video) video.srcObject = null;
  }

  /** The host ended the stream or refused it; a link that still works waits for the next one. */
  function over(from: GuestStream, why: Ending) {
    if (stream !== from || gone) return;
    stopped();
    if (why.wait) {
      waiting = why.message;
      phase = 'waiting';
      retry = setTimeout(join, WAIT_MS);
      return;
    }
    ended = why;
    phase = 'ended';
  }

  function leave() {
    clearTimeout(retry);
    stream?.leave();
    stopped();
    ended = { code: 'left', message: 'You left the stream.', wait: false, final: false };
    phase = 'ended';
  }

  /** The bar goes with the picture into full screen, so the stage is what fills the screen. */
  function toggleFullscreen() {
    if (document.fullscreenElement) void document.exitFullscreen().catch(() => {});
    else player?.requestFullscreen?.().catch(() => {});
    surface?.focus();
  }

  function toggleMute() {
    if (!video) return;
    video.muted = muted = !muted;
  }

  /** Shows the bar, and hides it again after a while unless the pointer or the focus is on it. */
  function wake() {
    barShown = true;
    clearTimeout(barTimer);
    barTimer = setTimeout(() => (barShown = barHeld), BAR_MS);
  }

  function hold(held: boolean) {
    barHeld = held;
    wake();
  }

  // A guest who only watches gets the bar on any move, like a video player. One whose pointer is
  // the game's gets it at the top edge, so it stays out of the game.
  function moved(event: PointerEvent) {
    if (!typing || event.pointerType !== 'mouse' || event.clientY - (player?.getBoundingClientRect().top ?? 0) < BAR_ZONE) wake();
  }

  // Keys and the mouse go to the host only with that permission, and while the picture has the focus.
  function key(event: KeyboardEvent, down: boolean) {
    if (phase !== 'streaming' || !typing) return;
    // Ctrl+Alt+Shift+Z gives the keyboard back to the page, as in Moonlight.
    if (down && event.ctrlKey && event.altKey && event.shiftKey && event.code === 'KeyZ') {
      surface?.blur();
      wake();
      return;
    }
    event.preventDefault();
    stream?.send(keyMessage(event.code, down));
  }

  $effect(() => {
    if (video && joined) return cropPadding(video, { width: joined.width, height: joined.height });
  });

  function pointer(event: PointerEvent) {
    if (phase !== 'streaming' || !typing || !video) return;
    if (event.type === 'pointerdown') {
      event.preventDefault();
      surface?.focus();
      surface?.setPointerCapture(event.pointerId);
    }
    const size = { width: video.videoWidth, height: video.videoHeight };
    const shown = joined ? shownSize(size, joined) : size;
    stream?.send(positionMessage(event.clientX, event.clientY, video.getBoundingClientRect(), size, shown));
    pressed(event.buttons);
  }

  function pressed(mask: number) {
    for (const change of buttonChanges(buttons, mask)) stream?.send(change);
    buttons = mask;
  }

  function wheel(event: WheelEvent) {
    if (phase !== 'streaming' || !typing) return;
    event.preventDefault();
    stream?.send(wheelInput.message(event.deltaX, event.deltaY, event.deltaMode));
  }

  function blurred() {
    stream?.releaseAll();
    buttons = 0;
  }

  onDestroy(() => {
    gone = true;
    clearTimeout(retry);
    clearTimeout(barTimer);
    if (!stream) return;
    stream.leave();
    stopped();
  });
</script>

<!-- Leaving the page ends the stream and lets go of its tracks. -->
<svelte:window
  onpagehide={() => {
    stream?.leave();
    stopped();
  }}
/>
<svelte:document onfullscreenchange={() => (fullscreen = !!document.fullscreenElement)} />

<main class="media-plane" class:live={phase === 'streaming'}>
  {#if phase !== 'streaming'}
    <GuestJoinCard phase={token ? phase : 'incomplete'} bind:name {waiting} {ended} {permission} onjoin={join} onleave={leave} />
  {/if}

  {#if token}
    <section class="player" bind:this={player} hidden={phase !== 'streaming'} aria-label="The host's stream" onpointermove={moved} onpointerdown={moved}>
      <!-- The picture takes every key and pointer event while it has the focus, when the invite allows them. -->
      <!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
      <div
        class="surface"
        bind:this={surface}
        tabindex="0"
        role="application"
        aria-label="Stream picture"
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
        <video id="guest-video" bind:this={video} autoplay playsinline></video>
      </div>
      {#if paused}
        <div class="paused">
          <Button variant="primary" icon="play" onclick={() => video?.srcObject instanceof MediaStream && play(video.srcObject)}>Play</Button>
        </div>
      {/if}
      <!-- The stream's bar: solid, never a blur over the picture, and gone once nobody reaches for it. -->
      <div
        class="bar"
        class:shown={barShown || paused}
        role="toolbar"
        aria-label="Stream controls"
        tabindex="-1"
        onfocusin={() => hold(true)}
        onfocusout={() => hold(false)}
        onpointerenter={() => hold(true)}
        onpointerleave={() => hold(false)}
      >
        {#if joined}
          <span class="who">
            <span class="name">{joined.name}</span>
            <Badge tone={joined.permission === 'view' ? 'neutral' : 'accent'}>{PERMISSION_TEXT[joined.permission] ?? joined.permission}</Badge>
          </span>
          <span class="mode">{joined.codec} · {joined.width}×{joined.height} · {joined.fps} fps</span>
        {/if}
        <span class="spacer"></span>
        {#if typeof player?.requestFullscreen === 'function'}
          <Button icon="fullscreen" onclick={toggleFullscreen}>{fullscreen ? 'Exit full screen' : 'Full screen'}</Button>
        {/if}
        <Button icon={muted ? 'mute' : 'sound'} onclick={toggleMute}>{muted ? 'Unmute' : 'Mute'}</Button>
        <Button icon="signout" onclick={leave}>Leave</Button>
      </div>
    </section>
  {/if}
</main>

<style>
  :global(body) {
    background: var(--media-bg);
  }
  main {
    display: grid;
    align-content: center;
    justify-items: center;
    min-height: 100vh;
    box-sizing: border-box;
    padding: var(--space-6) var(--space-4);
    background-image: var(--media-glow);
    background-repeat: no-repeat;
  }
  main.live {
    padding: 0;
  }
  .player {
    position: fixed;
    inset: 0;
    background: var(--media-black);
  }
  .player[hidden] {
    display: none;
  }
  .surface {
    position: absolute;
    inset: 0;
    overflow: hidden;
    outline: none;
    touch-action: none;
  }
  video {
    display: block;
    width: 100%;
    height: 100%;
    object-fit: contain;
    pointer-events: none;
  }
  .paused {
    position: absolute;
    inset: 0;
    z-index: var(--z-base);
    display: grid;
    place-items: center;
    background: var(--media-scrim-soft);
  }
  .bar {
    position: absolute;
    top: max(var(--space-3), env(safe-area-inset-top));
    right: max(var(--space-3), env(safe-area-inset-right));
    left: max(var(--space-3), env(safe-area-inset-left));
    z-index: var(--z-raised);
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-2) var(--space-3);
    max-width: 60rem;
    margin: 0 auto;
    padding: var(--space-2) var(--space-2) var(--space-2) var(--space-4);
    border: 1px solid var(--media-line);
    border-radius: var(--radius-lg);
    background: var(--media-panel);
    box-shadow: var(--media-edge), var(--media-shadow);
    opacity: 0;
    transform: translateY(calc(-100% - var(--space-4)));
    pointer-events: none;
    transition:
      opacity var(--duration-base) var(--ease-standard),
      transform var(--duration-base) var(--ease-standard);
  }
  .bar.shown,
  .bar:focus-within {
    opacity: 1;
    transform: none;
    pointer-events: auto;
  }
  /* Reduced motion: the bar fades in place instead of sliding in. */
  @media (prefers-reduced-motion: reduce) {
    .bar {
      transform: none;
      transition: none;
    }
  }
  .who {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    min-width: 0;
  }
  .name {
    overflow: hidden;
    font-weight: var(--weight-semibold);
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .mode {
    color: var(--media-idle);
    font-size: var(--text-sm);
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }
  .spacer {
    flex: 1;
  }
  /* A tablet drops the mode line; a phone puts the guest above the buttons. */
  @media (max-width: 900px) {
    .mode {
      display: none;
    }
  }
  @media (max-width: 560px) {
    .bar {
      padding: var(--space-2);
    }
    .who {
      flex: 1 1 100%;
      padding: var(--space-1) var(--space-2) 0;
    }
    .spacer {
      display: none;
    }
    .bar :global(.button) {
      flex: 1 1 auto;
      min-width: 0;
      padding-inline: var(--space-2);
      white-space: nowrap;
    }
  }
</style>
