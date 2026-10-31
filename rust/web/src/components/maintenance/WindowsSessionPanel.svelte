<script lang="ts">
  import { onMount } from 'svelte';
  import Button from '../Button.svelte';
  import Panel from '../Panel.svelte';
  import { api, type ConsoleRecovery } from '../../lib/api';
  import { confirm, failed, notify } from '../../lib/feedback.svelte';
  import { reason } from './util';

  let recovery = $state<ConsoleRecovery | null>(null);
  /** Checking the status and returning the desktop are separate: only the one running spins. */
  let checking = $state(false);
  let returning = $state(false);
  let error = $state('');
  let requested = $state(false);

  async function refresh() {
    if (checking) return;
    checking = true;
    try {
      recovery = (await api.sessions.console()).recovery;
      error = '';
    } catch (cause) {
      error = reason(cause);
      recovery = null;
    } finally {
      checking = false;
    }
  }

  async function returnToConsole() {
    const selected = recovery;
    if (!selected?.can_reattach || checking || returning) return;
    const ok = await confirm({
      title: 'Return this RDP desktop to the PC?',
      message: 'The desktop becomes visible on the PC’s physical console and its RDP connection disconnects. Rubylight only moves its own Windows session. The console must have no signed-in user, or contain a disconnected session verified to belong to the same user. Active sessions and other users are protected. Disconnect Moonlight streams first.',
      confirm: 'Return to console',
    });
    if (!ok) return;
    returning = true;
    try {
      await api.sessions.returnToConsole(selected);
      requested = true;
      recovery = null;
      notify('Windows was asked to return the desktop to the console.', 'ok');
    } catch (cause) {
      failed('Returning to the console failed', cause);
    } finally {
      returning = false;
    }
  }

  onMount(refresh);
</script>

<Panel title="Windows session" level={3} description="After using Remote Desktop, return this PC's desktop to its screen before reconnecting Moonlight.">
  <div class="stack">
    {#if error}
      <p class="notice danger" role="alert">Could not check the Windows session: {error}</p>
    {/if}
    {#if requested}
      <p class="notice" role="status">Return requested. Windows may restart the host while switching sessions. Refresh the status once the host is back, then reconnect Moonlight.</p>
    {/if}
    <div class="line">
      <p class="state" class:muted={!recovery?.can_reattach} role="status">{recovery ? recovery.reason : checking ? 'Checking the Windows session…' : ''}</p>
      <div class="row">
        <!-- Offered only when Windows allows it; the reason beside it says why not. -->
        {#if recovery?.can_reattach}
          <Button variant="primary" busy={returning} disabled={checking} onclick={returnToConsole}>Return to console</Button>
        {/if}
        <Button icon="refresh" busy={checking} disabled={returning} onclick={refresh}>Refresh status</Button>
      </div>
    </div>
  </div>
</Panel>

<style>
  .line {
    display: flex;
    align-items: center;
    justify-content: space-between;
    flex-wrap: wrap;
    gap: var(--space-3) var(--space-5);
  }
  .state {
    flex: 1 1 280px;
    font-size: var(--text-sm);
  }
</style>
