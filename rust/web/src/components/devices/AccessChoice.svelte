<script lang="ts">
  // What a device may do, as one choice of tier cards: Play games, Watch only or Everything, and
  // on the device page also Custom, which opens the single permissions.
  import Icon from '../Icon.svelte';
  import { ACCESS_CHOICES, type Access } from './access';

  let {
    value,
    legend,
    custom = false,
    recommend = false,
    onchange,
  }: {
    value: Access | 'custom' | 'none';
    legend: string;
    /** Offer Custom as a fourth choice. */
    custom?: boolean;
    /** Mark Play games as recommended. */
    recommend?: boolean;
    onchange: (value: Access | 'custom') => void;
  } = $props();

  const name = $props.id();
  const ICONS = { play: 'gamepad', view: 'monitor', all: 'key', custom: 'settings' } as const;
  const choices = $derived([
    ...ACCESS_CHOICES.map((choice) => ({ ...choice, recommended: recommend && choice.id === 'play' })),
    ...(custom ? [{ id: 'custom' as const, label: 'Custom', hint: 'Pick each permission below.', recommended: false }] : []),
  ]);
</script>

<fieldset class="access">
  <legend>{legend}</legend>
  <div class="choices">
    {#each choices as choice (choice.id)}
      <label class="choice" class:on={value === choice.id}>
        <input type="radio" {name} value={choice.id} checked={value === choice.id} onchange={() => onchange(choice.id)} />
        <span class="well" aria-hidden="true"><Icon name={ICONS[choice.id]} size={20} /></span>
        <span class="text">
          <span class="label">{choice.label}{#if choice.recommended}<span class="tag">Recommended</span>{/if}</span>
          <span class="hint">{choice.hint}</span>
        </span>
        <span class="tick" aria-hidden="true"><Icon name="check" size={14} /></span>
      </label>
    {/each}
  </div>
</fieldset>

<style>
  .access { display: grid; gap: var(--space-2); margin: 0; padding: 0; border: 0; min-width: 0; }
  legend { padding: 0; margin-bottom: var(--space-2); font-weight: var(--weight-semibold); font-size: var(--text-sm); }
  .choices { display: grid; grid-template-columns: repeat(auto-fit, minmax(220px, 1fr)); gap: var(--space-3); }
  .choice {
    position: relative;
    display: grid;
    grid-template-columns: auto minmax(0, 1fr) auto;
    align-items: start;
    gap: var(--space-3);
    padding: var(--space-4);
    border: 1px solid var(--line);
    border-radius: var(--radius-lg);
    background: var(--panel);
    box-shadow: var(--edge);
    cursor: pointer;
    transition: border-color var(--duration-fast) var(--ease-standard), background-color var(--duration-fast) var(--ease-standard);
  }
  .choice:hover { border-color: var(--line-strong); }
  .choice.on { border-color: var(--accent); background: var(--accent-soft); }
  /* The radio is the whole card: it takes the click and the keys, the tick shows the choice. */
  .choice input { position: absolute; inset: 0; width: 100%; height: 100%; margin: 0; opacity: 0; cursor: pointer; }
  .choice:has(input:focus-visible) { outline: var(--focus-width) solid var(--focus); outline-offset: var(--focus-offset); }
  .well { display: grid; place-items: center; width: 40px; height: 40px; border: 1px solid var(--line); border-radius: var(--radius); background: var(--sunken); color: var(--ink-2); }
  .on .well { border-color: var(--accent); background: var(--accent); color: var(--accent-ink); }
  .text { display: grid; gap: var(--space-1); min-width: 0; }
  .label { display: flex; flex-wrap: wrap; align-items: center; gap: var(--space-1) var(--space-2); font-weight: var(--weight-semibold); }
  .tag { padding: 1px var(--space-2); border: 1px solid var(--ok-line); border-radius: var(--radius-pill); background: var(--ok-soft); color: var(--ok); font-size: var(--text-xs); font-weight: var(--weight-medium); }
  .hint { color: var(--ink-2); font-size: var(--text-sm); line-height: var(--leading-snug); }
  .tick { display: grid; place-items: center; width: 22px; height: 22px; border: 1px solid var(--line-strong); border-radius: 50%; background: var(--field); color: transparent; }
  .on .tick { border-color: var(--accent); background: var(--accent); color: var(--accent-ink); }
  @media (forced-colors: active) {
    .choice.on { outline: 2px solid Highlight; }
  }
</style>
