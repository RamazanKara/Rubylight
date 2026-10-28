<script lang="ts">
  import Button from './Button.svelte';

  /** `slow`: why a first load can take a while, shown once it has run past a few seconds. */
  let { error, what, stale = false, slow, onretry }: {
    error: string;
    what: string;
    stale?: boolean;
    slow?: string;
    onretry: () => Promise<void>;
  } = $props();
  let retrying = $state(false);
  let lingering = $state(false);
  $effect(() => {
    lingering = false;
    if (error || !slow) return;
    const timer = setTimeout(() => (lingering = true), 3000);
    return () => clearTimeout(timer);
  });

  async function retry() {
    if (retrying) return;
    retrying = true;
    try {
      await onretry();
    } finally {
      retrying = false;
    }
  }
</script>

{#if error}
  <div class="notice danger failure" role="alert">
    <span>Could not {stale ? 'refresh' : 'load'} {what}: {error}{#if stale}. Showing the last known state.{/if}</span>
    <Button size="sm" busy={retrying} onclick={retry}>Retry<span class="visually-hidden"> {what}</span></Button>
  </div>
{:else}
  <p class="muted loading" role="status">Loading {what}…{#if lingering}{' '}{slow}{/if}</p>
{/if}

<style>
  .failure {
    align-items: center;
    justify-content: space-between;
    flex-wrap: wrap;
  }
  .loading {
    font-size: var(--text-sm);
  }
</style>
