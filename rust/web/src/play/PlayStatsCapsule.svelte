<script lang="ts">
  /**
   * The play page's stats capsule: frame rate, bitrate, round trip and codec in a corner of the
   * picture, in tabular figures so nothing jumps as they change.
   *
   * Opaque media tokens and no blur: it sits over live video. The numbers are the browser's own
   * (`lib/webrtc/stats.ts`); a dash stands for one it does not have yet.
   */
  import { statText, type StreamStats } from '../lib/webrtc/stats';

  let { stats, codec }: { stats: StreamStats | null; /** What the page asked for, until the browser names what it decodes. */ codec: string } = $props();

  const picture = $derived(stats?.height ? `${stats.codec || codec} ${stats.height}p` : codec);
</script>

<!-- It changes every second: a screen reader reads it when asked, not as it ticks. -->
<dl class="capsule" aria-label="Stream statistics" aria-live="off">
  <div>
    <dt>FPS</dt>
    <dd>{statText(stats?.fps ?? null)}</dd>
  </div>
  <div>
    <dt>Mbps</dt>
    <dd>{statText(stats?.mbps ?? null, 1)}</dd>
  </div>
  <div>
    <dt>RTT</dt>
    <dd>{statText(stats?.rttMs ?? null)}<span class="unit">ms</span></dd>
  </div>
  <div class="picture">
    <dt class="visually-hidden">Picture</dt>
    <dd>{picture}</dd>
  </div>
</dl>

<style>
  .capsule {
    position: absolute;
    top: max(3vh, env(safe-area-inset-top));
    right: max(3vw, env(safe-area-inset-right));
    z-index: var(--z-raised);
    display: flex;
    align-items: stretch;
    margin: 0;
    border: 1px solid var(--media-line);
    border-radius: var(--radius);
    background: var(--media-panel);
    box-shadow: var(--media-shadow);
    color: var(--media-ink);
    font-size: 0.7em;
    font-variant-numeric: tabular-nums;
    line-height: var(--leading-tight);
    pointer-events: none;
  }
  .capsule > div {
    display: grid;
    gap: 0.15em;
    justify-items: start;
    min-width: 3.4em;
    padding: 0.45em 0.8em;
  }
  .capsule > div + div {
    border-left: 1px solid var(--media-line);
  }
  dt {
    color: var(--media-idle);
    font-size: 0.75em;
    font-weight: var(--weight-semibold);
    letter-spacing: var(--tracking-label);
    text-transform: uppercase;
  }
  dd {
    margin: 0;
    font-weight: var(--weight-bold);
    white-space: nowrap;
  }
  .unit {
    margin-left: 0.2em;
    color: var(--media-idle);
    font-weight: var(--weight-medium);
  }
  .picture {
    align-content: center;
  }
  .picture dd {
    color: var(--media-ink-2);
    font-weight: var(--weight-semibold);
  }
</style>
