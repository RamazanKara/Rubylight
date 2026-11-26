<script lang="ts" module>
  import type { Permission } from '../lib/webrtc/guest';

  /** What an invite lets its guest do, as the page's badge says it. */
  export const PERMISSION_TEXT: Readonly<Record<Permission, string>> = {
    view: 'View only',
    controller: 'Controller',
    'controller-keyboard-mouse': 'Controller, keyboard and mouse',
  };
</script>

<script lang="ts">
  /**
   * The guest page's join card, alone on the stage until the picture plays: the name the host
   * will see, a calm wait while the host is not streaming, and a plain sentence when the stream
   * stopped or the link opens nothing any more.
   *
   * It shows what the host said, never a browser's WebRTC error: that one goes under "Details".
   */
  import BrandMark from '../components/BrandMark.svelte';
  import Badge from '../components/Badge.svelte';
  import Button from '../components/Button.svelte';
  import Field from '../components/Field.svelte';
  import Input from '../components/Input.svelte';
  import type { Ending } from '../lib/webrtc/guest';

  let {
    phase,
    name = $bindable(),
    waiting = '',
    ended = null,
    permission = null,
    onjoin,
    onleave,
  }: {
    phase: 'incomplete' | 'name' | 'joining' | 'waiting' | 'ended';
    /** The guest's name, as typed. */
    name?: string | number | null;
    /** What the host said while it does not stream. */
    waiting?: string;
    ended?: Ending | null;
    /** What the invite allowed when the host last offered the stream; unknown before that. */
    permission?: Permission | null;
    onjoin: () => void;
    onleave: () => void;
  } = $props();

  /** A browser's own error text follows the page's sentence after a colon; the card shows the sentence. */
  const browserFault = $derived(ended?.code === 'browser' ? ended.message.split(': ') : null);

  function submit(event: SubmitEvent) {
    event.preventDefault();
    onjoin();
  }
</script>

<section class="card" aria-labelledby="guest-title">
  <p class="brand"><BrandMark size={28} /><span>Rubylight</span></p>
  {#if phase === 'incomplete'}
    <h1 id="guest-title">This invite link is incomplete</h1>
    <p class="muted">The part after <code>#</code> is missing or cut short. Ask the host to send the link again.</p>
  {:else if phase === 'name'}
    <form onsubmit={submit}>
      <h1 id="guest-title">Join the stream</h1>
      <p class="muted">You’re invited to a game on someone’s PC. The host sees the name you pick.</p>
      <Field label="Your name" id="guest-name">
        <Input id="guest-name" bind:value={name} maxlength={32} autocomplete="nickname" placeholder="Guest" />
      </Field>
      <Button variant="primary" type="submit" icon="play">Join</Button>
    </form>
  {:else if phase === 'joining'}
    <div class="wait">
      <span class="rings" aria-hidden="true"></span>
      <h1 id="guest-title" role="status">Joining…</h1>
    </div>
  {:else if phase === 'waiting'}
    <div class="wait">
      <span class="rings" aria-hidden="true"></span>
      <h1 id="guest-title">Waiting for the host</h1>
      <p class="muted" role="status">{waiting || 'The host is not streaming right now. This page joins when the stream starts.'}</p>
      {#if permission}<Badge tone="neutral">{PERMISSION_TEXT[permission] ?? permission}</Badge>{/if}
      <Button variant="secondary" onclick={onleave}>Leave</Button>
    </div>
  {:else if ended}
    <h1 id="guest-title">{ended.code === 'left' ? 'You left' : ended.final ? 'This link no longer works' : 'The stream stopped'}</h1>
    <p role="alert">{browserFault ? `${browserFault[0]}. Try another browser, or update this one.` : ended.message}</p>
    {#if browserFault && browserFault.length > 1}
      <details>
        <summary>Details</summary>
        <p class="muted">{browserFault.slice(1).join(': ')}</p>
      </details>
    {/if}
    {#if !ended.final}<Button variant="primary" icon="refresh" onclick={onjoin}>Join again</Button>{/if}
  {/if}
</section>

<style>
  .card {
    display: grid;
    gap: var(--space-4);
    width: min(26rem, 100%);
    box-sizing: border-box;
    padding: var(--space-6);
    border: 1px solid var(--media-line);
    border-radius: var(--radius-xl);
    background-color: var(--media-panel);
    background-image: var(--media-glow);
    box-shadow: var(--media-edge), var(--media-shadow);
  }
  .brand {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    margin: 0 0 var(--space-2);
    color: var(--media-ink-2);
    font-weight: var(--weight-semibold);
  }
  form,
  .wait {
    display: grid;
    gap: var(--space-4);
  }
  .wait {
    justify-items: center;
    text-align: center;
  }
  h1 {
    margin: 0;
    font-family: var(--font-display);
    font-size: var(--text-xl);
    font-weight: var(--weight-bold);
    letter-spacing: var(--tracking-display);
    line-height: var(--leading-tight);
  }
  p {
    margin: 0;
    line-height: var(--leading);
  }
  code {
    padding: 0 var(--space-1);
    border-radius: var(--radius-xs);
    background: var(--media-panel-raised);
  }
  summary {
    color: var(--muted);
    font-size: var(--text-sm);
    cursor: pointer;
  }
  details p {
    margin-top: var(--space-2);
    font-size: var(--text-sm);
    overflow-wrap: anywhere;
  }
  /* A calm beacon while the page waits: one dot, one slow ring. Still under reduced motion. */
  .rings {
    position: relative;
    width: 12px;
    height: 12px;
    margin: var(--space-4);
    border-radius: var(--radius-pill);
    background: var(--media-accent-text);
  }
  .rings::after {
    content: '';
    position: absolute;
    inset: -14px;
    border: 1px solid var(--media-accent-text);
    border-radius: var(--radius-pill);
    opacity: 0.5;
    animation: ring calc(var(--pulse-duration) * 2) var(--ease-standard) infinite;
  }
  @keyframes ring {
    from {
      opacity: 0.6;
      transform: scale(0.4);
    }
    to {
      opacity: 0;
      transform: scale(1.3);
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .rings::after {
      animation: none;
    }
  }
  @media (max-width: 480px) {
    .card {
      padding: var(--space-5);
    }
  }
</style>
