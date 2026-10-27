<script lang="ts">
  // A placeholder in the shape of content that is loading. One label per
  // loading region: pass it on the outermost skeleton, the rest stay silent.
  let {
    shape = 'text',
    lines = 1,
    width,
    height,
    label,
  }: {
    shape?: 'text' | 'block' | 'circle';
    /** Text lines; the last one is shorter. */
    lines?: number;
    width?: string;
    height?: string;
    /** Announced to screen readers, e.g. "Loading devices". */
    label?: string;
  } = $props();
</script>

<span class="skeleton {shape}" role={label ? 'status' : undefined} aria-hidden={label ? undefined : 'true'}>
  {#if label}<span class="visually-hidden">{label}</span>{/if}
  {#if shape === 'text'}
    {#each Array.from({ length: Math.max(1, lines) }, (_, i) => i) as i (i)}
      <span class="bone line" style:width={i === lines - 1 && lines > 1 ? '60%' : width}></span>
    {/each}
  {:else}
    <span class="bone" style:width style:height></span>
  {/if}
</span>

<style>
  .skeleton {
    display: grid;
    gap: var(--space-2);
    min-width: 0;
  }
  .bone {
    display: block;
    background: var(--skeleton);
    border-radius: var(--radius-sm);
    animation: pulse var(--pulse-duration) ease-in-out infinite;
  }
  .line {
    height: 0.9em;
    width: 100%;
  }
  .block .bone {
    width: 100%;
    height: 120px;
    border-radius: var(--radius-lg);
  }
  .circle .bone {
    width: 40px;
    height: 40px;
    border-radius: 50%;
  }
  @keyframes pulse {
    50% {
      opacity: 0.55;
    }
  }
</style>
