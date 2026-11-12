<script lang="ts">
  import { onDestroy } from 'svelte';
  import StatusPill from '../StatusPill.svelte';
  import Button from '../Button.svelte';
  import Panel from '../Panel.svelte';
  import { api } from '../../lib/api';
  import { confirm, failed, notify } from '../../lib/feedback.svelte';
  import { backAfterRestart, confirmRestart } from './host';

  let phase = $state<'running' | 'restarting' | 'late' | 'stopped'>('running');
  let action = $state<'restart' | 'quit' | null>(null);
  let alive = true;
  onDestroy(() => (alive = false));

  async function restart() {
    if (action || phase === 'stopped') return;
    const ok = await confirmRestart();
    if (!ok || action) return;
    const previous = phase;
    action = 'restart';
    phase = 'restarting';
    try {
      await api.host.restart();
    } catch (error) {
      phase = previous;
      action = null;
      failed('Restart failed', error);
      return;
    }
    const back = await backAfterRestart(() => alive);
    action = null;
    if (back) {
      phase = 'running';
      notify('Rubylight restarted.', 'ok');
    } else if (alive) {
      phase = 'late';
    }
  }

  async function quit() {
    if (action || phase === 'stopped') return;
    const ok = await confirm({
      title: 'Quit Rubylight?',
      message:
        'Streaming stops and this console goes offline.\n\nWhen Rubylight runs as a service, Windows restarts it after a crash but not after Quit. It stays stopped until you start the service again or restart the PC.',
      confirm: 'Quit',
      danger: true,
    });
    if (!ok || action) return;
    action = 'quit';
    try {
      await api.host.quit();
      phase = 'stopped';
    } catch (error) {
      failed('Quit failed', error);
    } finally {
      action = null;
    }
  }
</script>

<Panel title="Rubylight" level={3}>
  {#snippet actions()}
    {#if phase === 'stopped'}
      <StatusPill status="error" label="Stopped" />
    {:else if phase === 'restarting'}
      <StatusPill status="busy" label="Restarting" />
    {:else if phase === 'late'}
      <StatusPill status="warning" label="Not responding" />
    {:else}
      <StatusPill status="ready" label="Running" />
    {/if}
  {/snippet}
  <div class="stack">
    {#if phase === 'restarting'}
      <p class="notice" role="status">Restarting. This console reconnects when Rubylight is back.</p>
    {:else if phase === 'late'}
      <p class="notice warn" role="alert">Rubylight has not come back after 90 seconds. Check the PC, then reload this page.</p>
    {:else if phase === 'stopped'}
      <p class="notice danger" role="status">Rubylight has stopped. This console is offline until Rubylight starts again.</p>
    {/if}
    <div class="line">
      <p class="muted note">Restart ends streams and applies settings that need it. Quit stops Rubylight until it is started again.</p>
      <div class="row">
        <Button icon="refresh" busy={action === 'restart'} disabled={phase === 'stopped' || action !== null} onclick={restart}>Restart</Button>
        <Button variant="danger" icon="power" busy={action === 'quit'} disabled={phase === 'stopped' || action !== null} onclick={quit}>Quit</Button>
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
  .note {
    flex: 1 1 280px;
    font-size: var(--text-sm);
  }
</style>
