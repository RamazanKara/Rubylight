<script lang="ts">
  /**
   * Quick Suspend's Suspend game button, on Home's running-app card and the live stage. Shown
   * only when the host says the running game can be paused now; a refusal is a toast with the
   * host's reason.
   */
  import Button from '../Button.svelte';
  import { api, type QuickSuspendStatus } from '../../lib/api';
  import { failed, notify } from '../../lib/feedback.svelte';
  import { canSuspend, outcome } from '../../lib/quick-suspend';

  let { quick, size, onchange }: { quick: QuickSuspendStatus | null; size?: 'sm'; onchange?: () => void } = $props();

  let suspending = $state(false);

  async function suspend() {
    suspending = true;
    try {
      const next = outcome(await api.sessions.suspend(), 'suspend');
      notify(next.text, next.ok ? 'ok' : 'danger');
      onchange?.();
    } catch (failure) {
      failed('Pausing the game failed', failure);
    } finally {
      suspending = false;
    }
  }
</script>

{#if canSuspend(quick)}
  <Button {size} icon="pause" busy={suspending} onclick={suspend}>Suspend game</Button>
{/if}
