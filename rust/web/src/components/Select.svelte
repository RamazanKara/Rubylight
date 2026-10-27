<script lang="ts" module>
  export interface Option {
    value: string;
    label: string;
    disabled?: boolean;
  }
</script>

<script lang="ts">
  // A native select, so keyboard, screen readers and touch work as the platform
  // expects. Put it in a Field for its label.
  import type { HTMLSelectAttributes } from 'svelte/elements';

  let {
    value = $bindable(''),
    options,
    invalid = false,
    describedby,
    onchange,
    ...rest
  }: Omit<HTMLSelectAttributes, 'value' | 'class' | 'onchange'> & {
    value?: string;
    options: Option[];
    invalid?: boolean;
    describedby?: string;
    onchange?: (value: string) => void;
  } = $props();
</script>

<select
  class="select"
  bind:value
  aria-invalid={invalid || undefined}
  aria-describedby={describedby}
  onchange={() => onchange?.(value)}
  {...rest}
>
  {#each options as option (option.value)}
    <option value={option.value} disabled={option.disabled}>{option.label}</option>
  {/each}
</select>

<style>
  .select[aria-invalid='true'] {
    border-color: var(--danger);
  }
</style>
