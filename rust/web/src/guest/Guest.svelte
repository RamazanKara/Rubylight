<script lang="ts">
  // The guest page (docs/browser-guests.md): its own small build under /guest/, with nothing of
  // the console's. The invite token is in the link's fragment; the page joins the owner's stream
  // with it and a name, plays picture and sound, and sends what the invite allows.
  import { onDestroy } from 'svelte';
  import BrandMark from '../components/BrandMark.svelte';
  import Button from '../components/Button.svelte';
  import Field from '../components/Field.svelte';
  import Input from '../components/Input.svelte';
  import { GuestStream, gamepads, inviteToken, keyboardMouse, type Ending, type Joined } from '../lib/webrtc/guest';
  import { buttonChanges, keyMessage, positionMessage, Wheel } from '../lib/webrtc/input';

  type Phase = 'name' | 'joining' | 'waiting' | 'streaming' | 'ended';

  const NAME_KEY = 'rubylight.guest.name';
  /** How often a waiting page asks again whether the owner streams. */
  const WAIT_MS = 5000;
  const PERMISSION_TEXT = {
    view: 'View only',
    controller: 'Controller',
    'controller-keyboard-mouse': 'Controller, keyboard and mouse',
  } as const;

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
    phase = 'streaming';
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

  function fullscreen() {
    surface?.requestFullscreen().catch(() => {});
    surface?.focus();
  }

  // Keys and the mouse go to the host only with that permission, and while the picture has the focus.
  function key(event: KeyboardEvent, down: boolean) {
    if (phase !== 'streaming' || !typing) return;
    // Ctrl+Alt+Shift+Z gives the keyboard back to the page, as in Moonlight.
    if (down && event.ctrlKey && event.altKey && event.shiftKey && event.code === 'KeyZ') {
      surface?.blur();
      return;
    }
    event.preventDefault();
    stream?.send(keyMessage(event.code, down));
  }

  function pointer(event: PointerEvent) {
    if (phase !== 'streaming' || !typing || !video) return;
    if (event.type === 'pointerdown') {
      event.preventDefault();
      surface?.focus();
      surface?.setPointerCapture(event.pointerId);
    }
    const size = { width: video.videoWidth, height: video.videoHeight };
    stream?.send(positionMessage(event.clientX, event.clientY, video.getBoundingClientRect(), size));
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

  function submit(event: SubmitEvent) {
    event.preventDefault();
    join();
  }

  onDestroy(() => {
    gone = true;
    clearTimeout(retry);
    if (!stream) return;
    stream.leave();
    stopped();
  });
</script>

<svelte:window onpagehide={() => stream?.leave()} />

<main class:live={phase === 'streaming'}>
  <header class="brand">
    <BrandMark size={28} />
    <span>Rubylight</span>
  </header>

  {#if !token}
    <section class="card">
      <h1>This invite link is incomplete</h1>
      <p class="muted">The part after <code>#</code> is missing or cut short.</p>
    </section>
  {:else if phase === 'name'}
    <form class="card" onsubmit={submit}>
      <h1>Join the stream</h1>
      <Field label="Your name" id="guest-name">
        <Input id="guest-name" bind:value={name} maxlength={32} autocomplete="nickname" placeholder="Guest" />
      </Field>
      <div><Button variant="primary" type="submit" icon="play">Join</Button></div>
    </form>
  {:else if phase === 'joining'}
    <section class="card"><p class="muted" role="status">Joining…</p></section>
  {:else if phase === 'waiting'}
    <section class="card">
      <h1>Waiting for the host</h1>
      <p class="muted" role="status">{waiting}</p>
      <div><Button variant="secondary" onclick={leave}>Leave</Button></div>
    </section>
  {:else if phase === 'ended' && ended}
    <section class="card">
      <h1>{ended.code === 'left' ? 'You left' : ended.final ? 'This link no longer works' : 'The stream stopped'}</h1>
      <p role="alert">{ended.message}</p>
      {#if !ended.final}<div><Button variant="primary" icon="refresh" onclick={join}>Join again</Button></div>{/if}
    </section>
  {/if}

  <section class="player" hidden={phase !== 'streaming'} aria-label="The host's stream">
    <div class="bar">
      {#if joined}
        <span class="name">{joined.name}</span>
        <span class="muted">{PERMISSION_TEXT[joined.permission] ?? joined.permission} · {joined.codec} {joined.width}×{joined.height} {joined.fps} fps</span>
      {/if}
      <span class="spacer"></span>
      <Button size="sm" icon="monitor" onclick={fullscreen}>Full screen</Button>
      <Button size="sm" variant="secondary" onclick={leave}>Leave</Button>
    </div>
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
      {#if paused}
        <div class="paused">
          <Button variant="primary" icon="play" onclick={() => video?.srcObject instanceof MediaStream && play(video.srcObject)}>Play</Button>
        </div>
      {/if}
    </div>
  </section>
</main>

<style>
  main {
    display: grid;
    gap: var(--space-5);
    align-content: start;
    justify-items: center;
    min-height: 100vh;
    box-sizing: border-box;
    padding: var(--space-6) var(--space-4);
  }
  main.live {
    justify-items: stretch;
    padding: var(--space-3);
    gap: var(--space-3);
  }
  .brand {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    font-weight: var(--weight-semibold);
  }
  .live .brand {
    display: none;
  }
  .card {
    display: grid;
    gap: var(--space-4);
    width: min(28rem, 100%);
    box-sizing: border-box;
    padding: var(--space-5);
    border: 1px solid var(--line);
    border-radius: var(--radius);
    background: var(--panel);
  }
  h1 {
    margin: 0;
    font-size: var(--text-lg);
  }
  .card p {
    margin: 0;
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
  .name {
    font-weight: var(--weight-semibold);
    overflow-wrap: anywhere;
  }
  .spacer {
    flex: 1;
  }
  .surface {
    position: relative;
    aspect-ratio: 16 / 9;
    max-height: calc(100vh - 5rem);
    background: #000;
    border-radius: var(--radius);
    overflow: hidden;
    touch-action: none;
  }
  .surface:focus-visible {
    outline: var(--focus-width) solid var(--focus);
    outline-offset: 2px;
  }
  .surface:fullscreen {
    border-radius: 0;
    max-height: none;
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
    display: grid;
    place-items: center;
    background: rgb(0 0 0 / 0.5);
  }
</style>
