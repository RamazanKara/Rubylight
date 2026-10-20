<script lang="ts">
  let {
    checked = $bindable(false),
    label,
    hint,
    disabled = false,
    onchange,
  }: {
    checked?: boolean;
    label: string;
    hint?: string;
    disabled?: boolean;
    onchange?: (checked: boolean) => void;
  } = $props();
</script>

<label class="toggle" class:disabled>
  <input
    type="checkbox"
    role="switch"
    bind:checked
    {disabled}
    onchange={() => onchange?.(checked)}
  />
  <span class="track" aria-hidden="true"><span class="thumb"></span></span>
  <span class="text">
    <span class="label">{label}</span>
    {#if hint}<span class="hint">{hint}</span>{/if}
  </span>
</label>

<style>
  .toggle {
    display: flex;
    gap: var(--space-3);
    align-items: center;
    min-height: 44px;
    cursor: pointer;
  }
  .disabled {
    opacity: 0.55;
    cursor: default;
  }
  input {
    position: absolute;
    opacity: 0;
    width: 1px;
    height: 1px;
  }
  .track {
    flex: none;
    /* Matches the Settings page's switch: an outlined track when off. */
    width: 40px;
    height: 22px;
    border-radius: 11px;
    border: 1px solid var(--muted);
    background: var(--panel);
    position: relative;
    transition:
      background 0.12s,
      border-color 0.12s;
  }
  .thumb {
    position: absolute;
    top: 4px;
    left: 4px;
    width: 12px;
    height: 12px;
    border-radius: 50%;
    background: var(--ink-2);
    transition: transform 0.12s;
  }
  .toggle:not(.disabled):hover .track {
    border-color: var(--ink-2);
  }
  input:checked + .track {
    background: var(--accent);
    border-color: var(--accent);
  }
  input:checked + .track .thumb {
    background: var(--accent-ink);
    transform: translateX(18px);
  }
  input:focus-visible + .track {
    outline: 2px solid var(--focus);
    outline-offset: 2px;
  }
  .text {
    min-width: 0;
    display: grid;
    gap: 2px;
  }
  .label {
    font-weight: 560;
    font-size: var(--text-sm);
  }
  .hint {
    font-size: var(--text-xs);
    color: var(--muted);
    line-height: 1.45;
  }
</style>
