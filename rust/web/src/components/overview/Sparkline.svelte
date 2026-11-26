<script lang="ts">
  let {
    values,
    label,
    slots = 120,
    fit = false,
    area = false,
  }: {
    values: number[];
    /** Read out instead of the drawing. */
    label: string;
    /** Points across the full width; fewer values are drawn against the right edge. */
    slots?: number;
    /**
     * Draw the values' own range instead of zero to the top, so a steady figure still shows its
     * shape. The range is never narrower than a fifth of the highest value: noise stays small.
     */
    fit?: boolean;
    /** Shade the space under the line. */
    area?: boolean;
  } = $props();

  const width = 240;
  const height = 40;

  const recent = $derived(values.slice(-slots).map((value) => (Number.isFinite(value) ? Math.max(0, value) : 0)));
  const range = $derived.by(() => {
    const high = Math.max(0, ...recent);
    if (!fit || !recent.length) return { low: 0, high: Math.max(1, high) * 1.15 };
    const low = Math.min(...recent);
    const span = Math.max(high - low, high * 0.2, 0.001);
    const middle = (high + low) / 2;
    const floor = Math.max(0, middle - span * 0.6);
    return { low: floor, high: floor + span * 1.2 };
  });
  const xy = $derived(
    recent.map((value, index) => {
      const x = ((slots - recent.length + index) / (slots - 1)) * width;
      const y = height - 1 - ((value - range.low) / (range.high - range.low)) * (height - 2);
      return [x, Math.min(height - 1, Math.max(1, y))] as const;
    }),
  );
  const points = $derived(xy.map(([x, y]) => `${x.toFixed(1)},${y.toFixed(1)}`).join(' '));
  const shade = $derived.by(() => {
    const first = xy[0];
    const last = xy[xy.length - 1];
    return first && last && xy.length > 1 ? `${first[0].toFixed(1)},${height} ${points} ${last[0].toFixed(1)},${height}` : '';
  });
</script>

<svg viewBox="0 0 {width} {height}" preserveAspectRatio="none" role="img" aria-label={label}>
  {#if area && shade}<polygon points={shade} />{/if}
  <line x1="0" y1={height - 0.5} x2={width} y2={height - 0.5} vector-effect="non-scaling-stroke" />
  <polyline {points} vector-effect="non-scaling-stroke" />
</svg>

<style>
  svg {
    display: block;
    width: 100%;
    height: 40px;
    overflow: visible;
  }
  line {
    stroke: var(--line);
    stroke-width: 1;
  }
  polygon {
    fill: var(--ink-2);
    opacity: 0.12;
  }
  polyline {
    fill: none;
    stroke: var(--ink-2);
    stroke-width: 1.5;
    stroke-linejoin: round;
    stroke-linecap: round;
  }
</style>
