<script lang="ts">
  /**
   * The play page's "can't play" card: why the PC does not answer or refuses, in the page's own
   * words (Play.svelte's `failed`), and one way on: Try again.
   */
  let { message, onretry }: { message: string; onretry: () => void } = $props();
</script>

<div class="card">
  <p class="eyebrow">Not connected</p>
  <h1>Can’t play right now</h1>
  <p class="error" role="alert">{message}</p>
  <div><button class="action" data-nav onclick={onretry}>Try again</button></div>
</div>

<style>
  .card {
    display: grid;
    gap: 1em;
    justify-self: center;
    width: min(34em, 100%);
    box-sizing: border-box;
    padding: 2em;
    border: 1px solid var(--media-line);
    border-radius: var(--radius-xl);
    background: var(--media-panel);
    box-shadow: var(--media-edge), var(--media-shadow);
  }
  p {
    margin: 0;
  }
  .eyebrow {
    color: var(--media-warn);
    font-size: 0.7em;
    font-weight: var(--weight-semibold);
    letter-spacing: var(--tracking-label);
    text-transform: uppercase;
  }
  h1 {
    margin: 0;
    font-family: var(--font-display);
    font-size: 1.7em;
    letter-spacing: var(--tracking-display);
    line-height: var(--leading-tight);
  }
  .error {
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
</style>
