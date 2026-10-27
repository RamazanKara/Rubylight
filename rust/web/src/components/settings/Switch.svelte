<script lang="ts">
  // A bare on/off switch. The caller labels it (label for=, or a wrapping
  // label), so the switch can sit in the control column of a setting row.
  let {
    id,
    checked,
    describedby,
    onchange,
  }: {
    id?: string;
    checked: boolean;
    describedby?: string;
    onchange: (checked: boolean) => void;
  } = $props();
</script>

<span class="switch">
  <input
    {id}
    type="checkbox"
    role="switch"
    {checked}
    aria-describedby={describedby}
    onchange={(event) => onchange(event.currentTarget.checked)}
  />
  <span class="track" aria-hidden="true"><span class="thumb"></span></span>
</span>

<style>
  .switch {
    position: relative;
    display: inline-grid;
    flex: none;
    width: 40px;
    height: 22px;
  }
  input {
    /* Covers the track (and a larger touch target) so taps reach it. */
    position: absolute;
    inset: -11px -2px;
    width: calc(100% + 4px);
    height: calc(100% + 22px);
    margin: 0;
    opacity: 0;
    cursor: pointer;
    z-index: 1;
  }
  .track {
    position: relative;
    border-radius: 11px;
    border: 1px solid var(--muted);
    background: var(--panel);
    transition:
      background var(--duration-fast),
      border-color var(--duration-fast);
  }
  .thumb {
    position: absolute;
    top: 4px;
    left: 4px;
    width: 12px;
    height: 12px;
    border-radius: 50%;
    background: var(--ink-2);
    transition:
      transform var(--duration-fast),
      width var(--duration-fast);
  }
  input:hover + .track {
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
    outline: var(--focus-width) solid var(--focus);
    outline-offset: 2px;
  }
</style>
