<script lang="ts">
  /**
   * The live stage's traces: frame rate, bitrate and host frame time, each a figure over its last
   * two minutes. Drawn on a media plane; three across, two on a phone.
   */
  import Sparkline from './Sparkline.svelte';
  import { duration } from '../../lib/format';
  import type { Trace } from './studio-telemetry';

  let { traces, stale = false }: { traces: Trace[]; stale?: boolean } = $props();
</script>

<dl class="traces" class:stale>
  {#each traces as trace (trace.id)}
    <div class="trace {trace.id}">
      <dt>{trace.label}</dt>
      <dd class="figure"><strong class="num">{trace.now}</strong> {trace.unit}</dd>
      <dd class="sub">{trace.sub}</dd>
      {#if trace.values.length > 1}
        <dd class="spark">
          <Sparkline fit area values={trace.values} label="{trace.label} over the last {duration(trace.values.length)}" />
        </dd>
      {/if}
    </div>
  {/each}
</dl>

<style>
  .traces {
    display: grid;
    grid-template-columns: repeat(3, minmax(0, 1fr));
    gap: var(--space-3);
    margin: 0;
    min-width: 0;
  }
  .trace {
    display: grid;
    align-content: start;
    gap: 2px;
    min-width: 0;
    padding: var(--space-3) var(--space-4);
    border: 1px solid var(--media-line);
    border-radius: var(--radius);
    background: var(--media-panel);
    box-shadow: var(--media-edge);
  }
  dt {
    color: var(--media-ink-2);
    font-size: var(--text-xs);
    font-weight: var(--weight-semibold);
    letter-spacing: var(--tracking-label);
    text-transform: uppercase;
  }
  dd {
    margin: 0;
  }
  .figure {
    color: var(--media-ink-2);
    font-size: var(--text-sm);
    white-space: nowrap;
  }
  .figure strong {
    color: var(--media-ink);
    font-family: var(--mono);
    font-size: var(--text-xl);
    font-weight: var(--weight-semibold);
    letter-spacing: var(--tracking-tight);
    line-height: var(--leading-tight);
  }
  .sub {
    color: var(--media-ink-2);
    font-size: var(--text-xs);
  }
  /* Each trace in its own colour; the baseline stays quiet. */
  .spark {
    margin-top: var(--space-2);
    --line: var(--media-line);
  }
  .fps .spark {
    --ink-2: var(--media-viz-4);
  }
  .bitrate .spark {
    --ink-2: var(--media-viz-1);
  }
  .host .spark {
    --ink-2: var(--media-viz-2);
  }
  /* Old figures step back until the next answer. */
  .stale .figure strong,
  .stale .spark {
    opacity: 0.55;
  }
  @media (max-width: 700px) {
    .traces {
      grid-template-columns: repeat(2, minmax(0, 1fr));
      gap: var(--space-2);
    }
    .trace {
      padding: var(--space-3);
    }
    .figure strong {
      font-size: var(--text-lg);
    }
    /* Three traces in two columns: the last takes the row. */
    .trace:last-child:nth-child(odd) {
      grid-column: 1 / -1;
    }
  }
</style>
