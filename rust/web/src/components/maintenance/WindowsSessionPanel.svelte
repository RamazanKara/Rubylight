<script lang="ts">
  import { onMount } from 'svelte';
  import Button from '../Button.svelte';
  import Panel from '../Panel.svelte';
  import { api, type ConsoleRecovery } from '../../lib/api';
  import { confirm, failed, notify } from '../../lib/feedback.svelte';
  import { reason } from './util';

  let recovery = $state<ConsoleRecovery | null>(null);
  let busy = $state(false);
  let error = $state('');
  let requested = $state(false);

  async function refresh() {
    busy = true;
    try {
      recovery = (await api.sessions.console()).recovery;
      error = '';
    } catch (cause) {
      error = reason(cause);
      recovery = null;
    } finally {
      busy = false;
    }
  }

  async function returnToConsole() {
    const selected = recovery;
    if (!selected?.can_reattach || busy) return;
    const ok = await confirm({
      title: 'Return this RDP desktop to the PC?',
      message: 'The desktop becomes visible on the PC’s physical console and its RDP connection disconnects. Rubylight only moves its own Windows session. The console must have no signed-in user, or contain a disconnected session verified to belong to the same user. Active sessions and other users are protected. Disconnect Moonlight streams first.',
      confirm: 'Return to console',
    });
    if (!ok) return;
    busy = true;
    try {
      await api.sessions.returnToConsole(selected);
      requested = true;
      recovery = null;
      notify('Windows was asked to return the desktop to the console.', 'ok');
    } catch (cause) {
      failed('Returning to the console failed', cause);
    } finally {
      busy = false;
    }
  }

  onMount(refresh);
</script>

<Panel title="Windows session">
  <div class="stack">
    <p>After using Remote Desktop, return this host’s desktop to the physical console before reconnecting Moonlight.</p>
    {#if error}
      <p class="notice danger" role="alert">{error}</p>
    {/if}
    {#if requested}
      <p class="notice" role="status">Return requested. Windows may restart the host while switching sessions. Refresh the status once the host is back, then reconnect Moonlight.</p>
    {/if}
    {#if recovery}
      <p class="muted">{recovery.reason}</p>
    {/if}
    <div class="row">
      <Button busy={busy} disabled={!recovery?.can_reattach} onclick={returnToConsole}>Return to console</Button>
      <Button icon="refresh" disabled={busy} onclick={refresh}>Refresh status</Button>
    </div>
  </div>
</Panel>
