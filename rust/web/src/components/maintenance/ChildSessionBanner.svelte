<script lang="ts">
  // Child sessions were removed. The host puts back the Windows settings their
  // setup changed at its start; this says so only when that is not done yet.
  import { onMount, untrack } from 'svelte';
  import Banner from '../Banner.svelte';
  import Button from '../Button.svelte';
  import LoadState from '../LoadState.svelte';
  import { api, type ChildSessionStatus } from '../../lib/api';
  import { confirm, failed, notify } from '../../lib/feedback.svelte';
  import { reason } from './util';

  /** A status to show before the first load (the tests render with one). */
  let { initial = null }: { initial?: ChildSessionStatus | null } = $props();

  let status = $state<ChildSessionStatus | null>(untrack(() => initial));
  let loadError = $state('');
  let restoring = $state(false);

  async function load() {
    try {
      status = await api.childSession.status();
      loadError = '';
    } catch (error) {
      loadError = reason(error);
    }
  }

  async function restore() {
    const ok = await confirm({
      title: 'Restore the Windows settings?',
      message: 'Rubylight puts Remote Desktop, its firewall rules, the session frame rate and child sessions back as they were before the removed child-session feature changed them.',
      confirm: 'Restore again',
    });
    if (!ok) return;
    restoring = true;
    try {
      status = await api.childSession.restore();
      if (status.pending) failed('Restoring the Windows settings failed', status.problem ?? 'they are still changed');
      else notify('Windows settings restored.', 'ok');
    } catch (error) {
      failed('Restoring the Windows settings failed', reason(error));
    } finally {
      restoring = false;
    }
  }

  onMount(() => {
    if (!initial) load();
  });
</script>

{#if loadError}
  <LoadState error={loadError} what="whether Windows settings from the removed child-session feature are still changed" onretry={load} />
{:else if status?.pending}
  <Banner tone="danger" title="Windows settings from the removed child-session feature are still changed">
    {status.problem ?? 'The Rubylight service restores them at its next start.'}
    {#snippet actions()}
      <Button busy={restoring} onclick={restore}>Restore again</Button>
    {/snippet}
  </Banner>
{/if}
