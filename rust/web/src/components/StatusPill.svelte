<script lang="ts" module>
  export type Status = 'idle' | 'ready' | 'streaming' | 'busy' | 'warning' | 'error' | 'offline';

  /** The word shown when the caller gives none: never colour alone. */
  export const statusLabels: Record<Status, string> = {
    idle: 'Idle',
    ready: 'Ready',
    streaming: 'Streaming',
    busy: 'Working',
    warning: 'Needs attention',
    error: 'Error',
    offline: 'Offline',
  };
</script>

<script lang="ts">
  // The state of one thing (host, device, encoder, check) as a dot and a word.
  // Use Badge for tags and counts that are not states.
  let { status, label }: { status: Status; label?: string } = $props();
</script>

<span class="pill {status}" data-status={status}>
  <span class="dot" aria-hidden="true"></span>
  {label ?? statusLabels[status]}
</span>

<style>
  .pill {
    display: inline-flex;
    align-items: center;
    gap: var(--space-2);
    padding: 2px var(--space-3) 2px var(--space-2);
    border-radius: var(--radius-pill);
    border: 1px solid var(--line);
    background: var(--sunken);
    color: var(--ink-2);
    font-size: var(--text-xs);
    font-weight: var(--weight-semibold);
    line-height: var(--leading);
    white-space: nowrap;
  }
  .dot {
    width: 8px;
    height: 8px;
    flex: none;
    border-radius: 50%;
    background: var(--muted);
  }
  .ready,
  .streaming {
    background: var(--ok-soft);
    border-color: transparent;
    color: var(--ok);
  }
  .warning,
  .busy {
    background: var(--warn-soft);
    border-color: transparent;
    color: var(--warn);
  }
  .error {
    background: var(--danger-soft);
    border-color: transparent;
    color: var(--danger);
  }
  .ready .dot,
  .streaming .dot,
  .warning .dot,
  .busy .dot,
  .error .dot {
    background: currentColor;
  }
  .offline .dot {
    background: none;
    box-shadow: inset 0 0 0 1.5px var(--muted);
  }
  .streaming .dot {
    animation: pulse var(--pulse-duration) ease-in-out infinite;
  }
  .busy .dot {
    animation: pulse calc(var(--pulse-duration) / 2) ease-in-out infinite;
  }
  @keyframes pulse {
    50% {
      opacity: 0.35;
    }
  }
</style>
