<script lang="ts">
  // What a device may do, as one choice: Play games, Watch only or Everything, and on the
  // device page also Custom, which opens the single permissions.
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
  const choices = $derived([
    ...ACCESS_CHOICES.map((choice) => ({ ...choice, label: recommend && choice.id === 'play' ? `${choice.label} (recommended)` : choice.label })),
    ...(custom ? [{ id: 'custom' as const, label: 'Custom', hint: 'Pick each permission below.' }] : []),
  ]);
</script>

<fieldset class="access">
  <legend>{legend}</legend>
  <div class="choices">
    {#each choices as choice (choice.id)}
      <label class="choice" class:on={value === choice.id}>
        <input type="radio" {name} value={choice.id} checked={value === choice.id} onchange={() => onchange(choice.id)} />
        <span class="text"><span class="label">{choice.label}</span><span class="hint">{choice.hint}</span></span>
      </label>
    {/each}
  </div>
</fieldset>

<style>
  .access { display: grid; gap: var(--space-2); margin: 0; padding: 0; border: 0; min-width: 0; }
  legend { padding: 0; margin-bottom: var(--space-2); font-weight: var(--weight-semibold); font-size: var(--text-sm); }
  .choices { display: grid; grid-template-columns: repeat(auto-fit, minmax(180px, 1fr)); gap: var(--space-2); }
  .choice { display: flex; align-items: flex-start; gap: var(--space-2); padding: var(--space-3); border: 1px solid var(--line); border-radius: var(--radius); background: var(--panel); cursor: pointer; }
  .choice.on { border-color: var(--accent); background: var(--sunken); }
  .choice input { margin-top: 3px; flex: none; }
  .text { display: grid; gap: 2px; min-width: 0; }
  .label { font-weight: 500; }
  .hint { color: var(--muted); font-size: var(--text-sm); }
</style>
