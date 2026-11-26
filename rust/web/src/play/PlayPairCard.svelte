<script lang="ts">
  /**
   * The play page's pairing screen: the PIN this browser made, one digit to a cell and large
   * enough to read from the couch, with the three steps to take at the PC.
   *
   * The page waits for the PC's console to confirm the PIN; a wrong or expired one offers a new PIN.
   */
  let { pin, name, error, onretry }: { pin: string; /** This browser, as the PC's Devices page will name it. */ name: string; error: string; onretry: () => void } = $props();
</script>

<section class="pair" aria-labelledby="pair-title">
  <div class="lead">
    <p class="eyebrow">First visit</p>
    <h1 id="pair-title">Pair with your PC</h1>
    <ol class="steps">
      <li><span>Open the Rubylight console on your PC.</span></li>
      <li><span>Go to <strong>Devices</strong>.</span></li>
      <li><span>Enter this PIN.</span></li>
    </ol>
  </div>
  <div class="code">
    <p class="pin" class:spent={!!error} aria-label="PIN {pin.split('').join(' ')}">
      {#each pin.split('') as digit, index (index)}<span aria-hidden="true">{digit}</span>{/each}
    </p>
    {#if error}
      <p class="error" role="alert">{error}</p>
      <button class="action" data-nav onclick={onretry}>New PIN</button>
    {:else}
      <p class="waiting" role="status"><span class="pulse" aria-hidden="true"></span>Waiting for the PC…</p>
    {/if}
    <p class="device">This device: <strong>{name}</strong></p>
  </div>
</section>

<style>
  .pair {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    align-items: center;
    gap: 2em 4em;
    justify-self: center;
    width: min(56em, 100%);
    box-sizing: border-box;
    padding: 2.5em;
    border: 1px solid var(--media-line);
    border-radius: var(--radius-xl);
    background-color: var(--media-panel);
    background-image: var(--media-glow);
    box-shadow: var(--media-edge), var(--media-shadow);
  }
  .eyebrow {
    margin: 0 0 0.6em;
    color: var(--media-accent-text);
    font-size: 0.7em;
    font-weight: var(--weight-semibold);
    letter-spacing: var(--tracking-label);
    text-transform: uppercase;
  }
  h1 {
    margin: 0 0 0.7em;
    font-family: var(--font-display);
    font-size: 2.2em;
    font-weight: var(--weight-bold);
    letter-spacing: var(--tracking-display);
    line-height: var(--leading-tight);
  }
  .steps {
    display: grid;
    gap: 0.6em;
    margin: 0;
    padding: 0;
    list-style: none;
    counter-reset: step;
    color: var(--media-ink-2);
  }
  .steps li {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr);
    align-items: center;
    gap: 0.7em;
    counter-increment: step;
  }
  .steps li::before {
    content: counter(step);
    display: grid;
    place-items: center;
    width: 1.8em;
    height: 1.8em;
    border: 1px solid var(--media-line-strong);
    border-radius: var(--radius-pill);
    background: var(--media-panel-raised);
    color: var(--media-ink);
    font-size: 0.8em;
    font-weight: var(--weight-bold);
    font-variant-numeric: tabular-nums;
  }
  .steps strong {
    color: var(--media-ink);
  }
  .code {
    display: grid;
    justify-items: center;
    gap: 1em;
    min-width: 0;
  }
  .pin {
    display: flex;
    gap: 0.14em;
    margin: 0;
    font-family: var(--font-display);
    font-size: 4em;
    font-weight: var(--weight-bold);
    font-variant-numeric: tabular-nums;
    line-height: 1;
  }
  .pin span {
    display: grid;
    place-items: center;
    width: 1.05em;
    height: 1.4em;
    border: 1px solid var(--media-line-strong);
    border-radius: var(--radius);
    background: var(--media-bg);
    box-shadow: var(--media-edge);
  }
  /* A PIN the PC turned down or that ran out. */
  .pin.spent span {
    color: var(--media-idle);
    text-decoration: line-through;
  }
  p {
    margin: 0;
  }
  .waiting {
    display: flex;
    align-items: center;
    gap: 0.6em;
    color: var(--media-ink-2);
  }
  .pulse {
    width: 0.6em;
    height: 0.6em;
    border-radius: var(--radius-pill);
    background: var(--media-ok);
    box-shadow: var(--media-halo-ok);
    animation: breathe var(--pulse-duration) var(--ease-standard) infinite alternate;
  }
  @keyframes breathe {
    to {
      opacity: 0.35;
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .pulse {
      animation: none;
    }
  }
  .error {
    max-width: 22em;
    color: var(--media-danger);
    text-align: center;
  }
  .device {
    color: var(--media-idle);
    font-size: 0.8em;
  }
  .device strong {
    color: var(--media-ink-2);
  }
  .action {
    min-width: 10em;
    min-height: max(44px, 2.8em);
    padding: 0.5em 1.2em;
    border: 2px solid var(--media-accent);
    border-radius: var(--radius);
    background: var(--media-accent);
    color: var(--media-accent-ink);
    font: inherit;
    font-weight: var(--weight-semibold);
    cursor: pointer;
  }
  .action:focus {
    outline: 4px solid var(--media-focus);
    outline-offset: 4px;
  }
  @media (max-width: 900px) {
    .pair {
      grid-template-columns: minmax(0, 1fr);
      padding: 1.4em;
    }
    h1 {
      font-size: 1.7em;
    }
    .pin {
      font-size: 3.2em;
    }
  }
</style>
