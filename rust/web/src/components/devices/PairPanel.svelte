<script lang="ts">
  // Pairing, QR code first: Nightjar scans it; Moonlight, Artemis and other apps show a PIN
  // that is typed here. The one-time PIN for Artemis is under Other ways to pair. What the
  // device may do once paired is chosen first and sent with every code and PIN.
  import { onMount } from 'svelte';
  import Button from '../Button.svelte';
  import CopyButton from '../CopyButton.svelte';
  import Icon from '../Icon.svelte';
  import Panel from '../Panel.svelte';
  import AccessChoice from './AccessChoice.svelte';
  import OtpPairing from './OtpPairing.svelte';
  import PendingRequest from './PendingRequest.svelte';
  import QrPairing from './QrPairing.svelte';
  import type { Access } from './access';
  import { api } from '../../lib/api';
  import { failed } from '../../lib/feedback.svelte';
  import { link } from '../../lib/router.svelte';
  import { refreshMetadata, refreshPending, session } from '../../lib/session.svelte';

  let { title = 'Pair a device', bare = false }: { title?: string; bare?: boolean } = $props();

  const others = $derived((session.metadata?.pc_addresses ?? []).filter(address => address !== session.metadata?.pc_address));
  // On unless the settings file turns it off (`qr_pairing = false`).
  const qr = $derived(session.metadata ? session.metadata.qr_pairing !== false : false);
  const qrOff = $derived(session.metadata?.qr_pairing === false);
  // Pairing is approved here, by showing a code or typing a PIN, so the device may play.
  let access = $state<Access>('play');
  let turningOn = $state(false);
  onMount(() => { refreshMetadata().catch(() => {}); refreshPending().catch(() => {}); });

  async function turnOnQr() {
    turningOn = true;
    try {
      await api.config.patch({ qr_pairing: true });
      await refreshMetadata();
    } catch (error) {
      failed('Turning on pairing with a QR code failed', error);
    } finally {
      turningOn = false;
    }
  }
</script>

{#snippet body()}
  {#if session.metadata?.enable_pairing === false}
    <p class="notice">Pairing is off. Turn on Allow pairing in <a href="/settings" use:link>Settings</a> to pair a new device. Devices already paired keep working.</p>
  {:else}
    <div class="access">
      <AccessChoice legend="This device can" recommend value={access} onchange={(next) => { if (next !== 'custom') access = next; }} />
      <p class="muted">The first device paired to this PC can do everything. Change it later on the device's page.</p>
    </div>
    {#if qrOff}
      <p class="notice qr-off">Pairing with a QR code is off. <Button size="sm" busy={turningOn} onclick={turnOnQr}>Turn on</Button></p>
    {/if}
    <div class="ways" class:single={!qr}>
      {#if qr}
        <section class="way" aria-labelledby="pair-qr">
          <h3 id="pair-qr"><Icon name="image" size={18} />Nightjar: scan this code</h3>
          <p class="muted">In Nightjar, tap Add PC, then Scan QR code. On a computer, copy the link and paste it in Add PC.</p>
          <QrPairing {access} />
        </section>
      {/if}
      <section class="way" aria-labelledby="pair-pin">
        <h3 id="pair-pin"><Icon name="key" size={18} />{qr ? 'Moonlight or another app: type its PIN' : 'Type the PIN your device shows'}</h3>
        <ol class="steps">
          <li>
            <span class="step" aria-hidden="true">1</span>
            <div class="step-body">
              <p>Open the app on your device and select this PC.</p>
              {#if session.metadata}
                <div class="host-address"><Icon name="monitor" size={20} /><strong>{session.metadata.host_name}</strong><span class="num">{session.metadata.pc_address}</span><span class="copy"><CopyButton value={session.metadata.pc_address} label="Copy address" what="the address" /></span></div>
                {#if others.length}<details class="addresses"><summary>Other addresses</summary><p class="num">{others.join(', ')}</p></details>{/if}
              {:else}<p class="muted">Loading this PC's address…</p>{/if}
            </div>
          </li>
          <li>
            <span class="step" aria-hidden="true">2</span>
            <div class="step-body">
              <p>Type the PIN it shows here.</p>
              {#if session.pending.length}
                <ul class="requests" aria-label="Waiting for a PIN">{#each session.pending as request (request.uniqueid)}<PendingRequest {request} {access} />{/each}</ul>
              {:else}<p class="waiting muted" role="status">Waiting for a device. It appears here a few seconds after you select this PC.</p>{/if}
            </div>
          </li>
        </ol>
      </section>
    </div>
    <details class="alternatives"><summary>Other ways to pair</summary><p class="muted">Artemis: make a one-time PIN here and type it on the device.</p><OtpPairing {access} /></details>
  {/if}
{/snippet}

{#if bare}
  {@render body()}
{:else}
  <Panel {title}>{@render body()}</Panel>
{/if}

<style>
  .access { display: grid; gap: var(--space-2); margin-bottom: var(--space-4); }
  .access > p { font-size: var(--text-sm); }
  .qr-off { display: flex; align-items: center; flex-wrap: wrap; gap: var(--space-2) var(--space-3); margin-bottom: var(--space-4); }
  .ways { display: grid; grid-template-columns: minmax(0, 1fr) minmax(0, 1fr); gap: var(--space-5); }
  .ways.single { grid-template-columns: minmax(0, 1fr); max-width: 720px; }
  .way { display: grid; gap: var(--space-3); align-content: start; padding: var(--space-4) var(--space-5); border: 1px solid var(--line); border-radius: var(--radius-lg); background: var(--panel); }
  .way h3 { display: flex; align-items: center; gap: var(--space-2); font-size: var(--text-md); font-weight: var(--weight-semibold); }
  .way h3 :global(svg) { color: var(--accent-text); }
  .way > p { font-size: var(--text-sm); }
  .steps { display: grid; gap: var(--space-4); padding: 0; margin: 0; list-style: none; }
  .steps > li { display: flex; align-items: flex-start; gap: var(--space-3); }
  .step { flex: none; display: grid; place-items: center; width: 28px; height: 28px; border: 2px solid var(--line-strong); border-radius: 50%; font-weight: var(--weight-bold); font-size: var(--text-sm); }
  .step-body { flex: 1; min-width: 0; display: grid; gap: var(--space-2); padding-top: 2px; }
  .host-address { display: flex; align-items: center; flex-wrap: wrap; gap: var(--space-2) var(--space-3); padding: var(--space-3) var(--space-4); background: var(--sunken); border: 1px solid var(--line); border-radius: var(--radius); }
  .host-address strong { font-weight: 500; }
  .host-address .num { color: var(--ink-2); font-size: var(--text-sm); }
  .copy { display: inline-flex; align-items: center; margin-left: auto; }
  .requests { display: grid; gap: var(--space-3); list-style: none; padding: 0; margin: 0; }
  .waiting { padding: var(--space-4); background: var(--sunken); border: 1px dashed var(--line-strong); border-radius: var(--radius); font-size: var(--text-sm); }
  .alternatives { margin-top: var(--space-4); }
  .alternatives > p { margin: var(--space-3) 0; font-size: var(--text-sm); }
  .addresses { font-size: var(--text-sm); }
  @media (max-width: 900px) { .ways { grid-template-columns: minmax(0, 1fr); } }
  @media (max-width: 600px) { .way { padding: var(--space-4); } .copy { margin-left: 0; } }
</style>
