<script lang="ts">
  // A text input styled like every other control. Put it in a Field for its
  // label, hint and error; pass the same id to both.
  import type { HTMLInputAttributes } from 'svelte/elements';

  let {
    value = $bindable(''),
    invalid = false,
    mono = false,
    describedby,
    ...rest
  }: Omit<HTMLInputAttributes, 'value' | 'class'> & {
    value?: string | number | null;
    /** Marks the value as wrong; the Field shows why. */
    invalid?: boolean;
    /** Paths, addresses, keys. */
    mono?: boolean;
    describedby?: string;
  } = $props();
</script>

<input class="input" class:mono bind:value aria-invalid={invalid || undefined} aria-describedby={describedby} {...rest} />

<style>
  .input[aria-invalid='true'] {
    border-color: var(--danger);
  }
</style>
