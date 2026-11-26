<script lang="ts">
  /**
   * Where a new picture's time goes on this PC: one bar, a segment per phase, as wide as its share.
   * Drawn on a media plane. A phase the host did not time is not drawn; with none, nothing is.
   */
  import { shortMs, type Phase } from './studio-telemetry';

  let { phases }: { phases: Phase[] } = $props();

  const total = $derived(phases.reduce((sum, phase) => sum + phase.ms, 0));
  const summary = $derived(phases.map((phase) => `${phase.label} ${shortMs(phase.ms)} milliseconds`).join(', '));
</script>

{#if phases.length}
  <div class="timeline">
    <div class="head">
      <span class="eyebrow">Pipeline</span>
      <span class="total"><strong class="num">{shortMs(total)}</strong> ms from present to sent</span>
    </div>
    <div class="bar" role="img" aria-label="Time per new picture on this PC: {summary}">
      {#each phases as phase (phase.id)}
        <span class="segment {phase.id}" style:flex-grow={Math.max(phase.share, 0.04)}></span>
      {/each}
    </div>
    <ul class="legend" aria-hidden="true">
      {#each phases as phase (phase.id)}
        <li><span class="key {phase.id}"></span>{phase.label}<span class="num">{shortMs(phase.ms)} ms</span></li>
      {/each}
    </ul>
  </div>
{/if}

<style>
  .timeline {
    display: grid;
    gap: var(--space-2);
    min-width: 0;
  }
  .head {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    justify-content: space-between;
    gap: var(--space-1) var(--space-4);
  }
  .eyebrow {
    color: var(--media-ink-2);
  }
  .total {
    font-size: var(--text-sm);
    color: var(--media-ink-2);
  }
  .total strong {
    color: var(--media-ink);
    font-weight: var(--weight-semibold);
  }
  .bar {
    display: flex;
    gap: 3px;
    height: 12px;
  }
  .segment {
    flex: 1 1 0;
    min-width: 6px;
    border-radius: var(--radius-xs);
    transition: flex-grow var(--duration-slow) var(--ease-standard);
  }
  .segment:first-child {
    border-start-start-radius: var(--radius-pill);
    border-end-start-radius: var(--radius-pill);
  }
  .segment:last-child {
    border-start-end-radius: var(--radius-pill);
    border-end-end-radius: var(--radius-pill);
  }
  .capture {
    background: var(--media-viz-1);
  }
  .convert {
    background: var(--media-viz-2);
  }
  .encode {
    background: var(--media-viz-3);
  }
  .send {
    background: var(--media-viz-4);
  }
  .legend {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-1) var(--space-5);
    margin: 0;
    padding: 0;
    list-style: none;
    font-size: var(--text-xs);
    color: var(--media-ink-2);
  }
  .legend li {
    display: inline-flex;
    align-items: center;
    gap: var(--space-2);
  }
  .legend .num {
    color: var(--media-ink);
  }
  .key {
    width: 8px;
    height: 8px;
    border-radius: 50%;
  }
</style>
