<script lang="ts">
  /**
   * A labelled bar for an amount of a whole: the clips quota ("4.2 GB of 20 GB used") or how much
   * of the replay buffer is filled. The words carry the value; the bar only shows it.
   */
  let {
    label,
    text,
    share,
    tone = 'accent',
  }: {
    /** Names the bar for assistive technology. */
    label: string;
    /** The value in words, shown beside the label and read out. */
    text: string;
    /** 0 to 1. */
    share: number;
    tone?: 'accent' | 'warn' | 'danger' | 'live';
  } = $props();
  const percent = $derived(Math.round(Math.min(1, Math.max(0, share)) * 100));
</script>

<div class="dvr-meter">
  <div class="words">
    <span class="label">{label}</span>
    <span class="text num">{text}</span>
  </div>
  <div
    class="bar {tone}"
    role="meter"
    aria-label={label}
    aria-valuemin="0"
    aria-valuemax="100"
    aria-valuenow={percent}
    aria-valuetext={text}
  >
    <span class="fill" style:transform="scaleX({percent / 100})"></span>
  </div>
</div>

<style>
  .dvr-meter {
    display: grid;
    gap: var(--space-2);
    min-width: 0;
  }
  .words {
    display: flex;
    flex-wrap: wrap;
    justify-content: space-between;
    gap: var(--space-1) var(--space-3);
    font-size: var(--text-sm);
  }
  .label {
    color: var(--muted);
  }
  .text {
    font-weight: var(--weight-medium);
  }
  .bar {
    position: relative;
    block-size: 8px;
    border-radius: var(--radius-pill);
    background: var(--sunken);
    overflow: hidden;
  }
  .fill {
    position: absolute;
    inset: 0;
    background: var(--accent);
    transform-origin: left center;
    transition: transform var(--duration-base) var(--ease-standard);
  }
  .warn .fill {
    background: var(--warn);
  }
  .danger .fill {
    background: var(--danger);
  }
  .live .fill {
    background: var(--live);
  }
</style>
