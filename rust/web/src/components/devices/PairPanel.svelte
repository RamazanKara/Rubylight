<script lang="ts">
  import { onMount } from 'svelte';
  import CopyButton from '../CopyButton.svelte';
  import Icon from '../Icon.svelte';
  import Panel from '../Panel.svelte';
  import OtpPairing from './OtpPairing.svelte';
  import PendingRequest from './PendingRequest.svelte';
  import { link } from '../../lib/router.svelte';
  import { refreshMetadata, refreshPending, session } from '../../lib/session.svelte';
  const others = $derived((session.metadata?.pc_addresses ?? []).filter(address => address !== session.metadata?.pc_address));
  onMount(() => { refreshMetadata().catch(() => {}); refreshPending().catch(() => {}); });
</script>

<Panel title="Pair a device">
  {#if session.metadata?.enable_pairing === false}
    <p class="notice">Pairing is disabled. Turn on Allow pairing in <a href="/settings/general" use:link>General settings</a> to pair a new device. Devices already paired keep working.</p>
  {:else}
    <ol class="steps">
      <li><span class="step" aria-hidden="true">1</span><div class="step-body">
        <p>Open Moonlight, Nightjar or Artemis on your device and select this PC.</p>
        {#if session.metadata}
          <div class="host-address"><Icon name="monitor" size={24} /><strong>{session.metadata.host_name}</strong><span class="num">{session.metadata.pc_address}</span><span class="copy"><CopyButton value={session.metadata.pc_address} label="Copy address" what="the address" showLabel /></span></div>
          {#if others.length}<details class="addresses"><summary>Other addresses</summary><p class="num">{others.join(', ')}</p></details>{/if}
        {:else}<p class="muted">Loading this PC's address…</p>{/if}
      </div></li>
      <li><span class="step" aria-hidden="true">2</span><div class="step-body">
        <p>Enter the PIN shown on your device.</p>
        {#if session.pending.length}
          <ul class="requests" aria-label="Waiting for a PIN">{#each session.pending as request (request.uniqueid)}<PendingRequest {request} />{/each}</ul>
        {:else}<p class="waiting muted" role="status">No device is waiting to pair. Start pairing on your device; it appears here within a few seconds.</p>{/if}
      </div></li>
    </ol>
    <details class="alternatives"><summary>Other pairing methods</summary><p class="muted">Artemis only: create a one-time PIN here and enter it on the device.</p><OtpPairing /></details>
  {/if}
</Panel>

<style>
  .steps { display: grid; gap: 28px; padding: 0; margin: 0; list-style: none; }
  .steps > li { display: flex; align-items: flex-start; gap: var(--space-5); }
  .step { flex: none; display: grid; place-items: center; width: 36px; height: 36px; border: 2px solid var(--line-strong); border-radius: 50%; font-weight: var(--weight-bold); }
  .step-body { flex: 1; min-width: 0; display: grid; gap: var(--space-3); padding-top: var(--space-1); }
  .host-address { display: flex; align-items: center; flex-wrap: wrap; gap: var(--space-3) var(--space-5); padding: var(--space-4) 20px; background: var(--sunken); border: 1px solid var(--line); border-radius: var(--radius); }
  .host-address strong { font-weight: 500; }
  .host-address .num { color: var(--ink-2); font-size: var(--text-sm); }
  .copy { display: inline-flex; align-items: center; gap: var(--space-1); margin-left: auto; font-size: var(--text-sm); }
  .requests { display: grid; gap: var(--space-3); list-style: none; padding: 0; margin: 0; }
  .waiting { padding: 20px; background: var(--sunken); border: 1px dashed var(--line-strong); border-radius: var(--radius); }
  .alternatives { margin: var(--space-4) 0 0 60px; }
  .alternatives > p { margin-bottom: var(--space-4); font-size: var(--text-sm); }
  .addresses { font-size: var(--text-sm); }
  @media (max-width: 600px) { .steps > li { gap: var(--space-3); } .step { width: 28px; height: 28px; } .host-address { gap: var(--space-2); padding: var(--space-3); } .copy { margin-left: 0; } .alternatives { margin-left: 40px; } }
</style>
