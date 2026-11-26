<script lang="ts">
  /**
   * Quick Suspend's banner on Home: the paused game, since when and from which device, and
   * Resume game. Shown only while the host reports the game suspended (`/api/session/suspend`).
   */
  import Banner from '../Banner.svelte';
  import Button from '../Button.svelte';
  import { api, type QuickSuspendStatus } from '../../lib/api';
  import { failed, notify } from '../../lib/feedback.svelte';
  import { outcome, suspendedCopy } from '../../lib/quick-suspend';

  let { quick, onchange }: { quick: QuickSuspendStatus; onchange?: () => void } = $props();

  let resuming = $state(false);

  async function resume() {
    resuming = true;
    try {
      const next = outcome(await api.sessions.resume(), 'resume');
      notify(next.text, next.ok ? 'ok' : 'danger');
      onchange?.();
    } catch (failure) {
      failed('Resuming the game failed', failure);
    } finally {
      resuming = false;
    }
  }
</script>

<Banner tone="warn" title="Game suspended" live>
  {suspendedCopy(quick)}
  {#snippet actions()}
    <Button variant="primary" icon="play" busy={resuming || quick.state === 'resuming'} onclick={resume}>Resume game</Button>
  {/snippet}
</Banner>
