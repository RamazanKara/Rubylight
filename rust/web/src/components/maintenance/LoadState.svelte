<script lang="ts">
  import Button from '../Button.svelte';

  let { error, what, stale = false, onretry }: {
    error: string;
    what: string;
    stale?: boolean;
    onretry: () => Promise<void>;
  } = $props();
  let retrying = $state(false);

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
  <p class="muted loading" role="status">Loading {what}…</p>
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
