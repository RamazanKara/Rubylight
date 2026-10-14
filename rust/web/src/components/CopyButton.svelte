<script lang="ts">
  import { copyText } from '../lib/clipboard';
  import { failed, notify } from '../lib/feedback.svelte';
  import Icon from './Icon.svelte';

  let {
    value,
    label,
    what = value,
  }: {
    value: string;
    /** Read out and shown as the tooltip, such as "Copy address". */
    label: string;
    /** Named in the confirmation; the value itself by default. */
    what?: string;
  } = $props();

  let copied = $state(false);
  let timer: ReturnType<typeof setTimeout> | undefined;

  async function copy() {
    try {
      await copyText(value);
      notify(`Copied ${what}.`, 'ok', 2500);
      copied = true;
      clearTimeout(timer);
      timer = setTimeout(() => (copied = false), 1500);
    } catch (error) {
      failed('Copying failed', error);
    }
  }

  $effect(() => () => clearTimeout(timer));
</script>

<button type="button" class="copy" aria-label={label} title={label} onclick={copy}>
  <Icon name={copied ? 'check' : 'copy'} size={14} />
</button>

<style>
  .copy {
    flex: none;
    display: inline-grid;
    place-items: center;
    width: 26px;
    height: 26px;
    padding: 0;
    border: 1px solid transparent;
    border-radius: var(--radius);
    background: none;
    color: var(--muted);
    cursor: pointer;
    vertical-align: middle;
  }
  .copy:hover {
    color: var(--ink);
    background: var(--sunken);
    border-color: var(--line);
  }
  @media (max-width: 860px) {
    .copy {
      width: 44px;
      height: 44px;
    }
  }
</style>
