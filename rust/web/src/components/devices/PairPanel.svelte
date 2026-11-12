<script lang="ts">
  // Pairing as two tiles: Nightjar scans the QR code; Moonlight, Artemis and other apps show
  // a PIN that is typed here (Artemis's one-time PIN is under Other ways to pair). The first
  // device always gets Everything, so the access choice shows from the second device on and
  // is sent with every code and PIN. While pairing is off the code stays in its place,
  // faded behind the one click that allows it. When a device pairs, the QR tile becomes its card.
  import { onMount, tick, untrack } from 'svelte';
  import Button from '../Button.svelte';
  import CopyButton from '../CopyButton.svelte';
  import Icon from '../Icon.svelte';
  import Panel from '../Panel.svelte';
  import AccessChoice from './AccessChoice.svelte';
  import OtpPairing from './OtpPairing.svelte';
  import PendingRequest from './PendingRequest.svelte';
  import QrPairing from './QrPairing.svelte';
  import { accessLabel, type Access } from './access';
  import { knownDevices, newlyPaired } from './paired';
  import { pairLinkTarget } from './pairLink';
  import { api, type Client } from '../../lib/api';
  import { failed } from '../../lib/feedback.svelte';
  import { poll } from '../../lib/format';
  import { query, route } from '../../lib/router.svelte';
  import { refreshMetadata, refreshPending, session } from '../../lib/session.svelte';

  let {
    title = 'Pair a device',
    bare = false,
    clients,
    onpaired,
  }: {
    title?: string;
    bare?: boolean;
    /** The paired devices when the page polls them already; without them the panel polls the list itself, as often as Devices does. */
    clients?: Client[] | null;
    /** A device paired while the panel was open. */
    onpaired?: (client: Client) => void;
  } = $props();

  const others = $derived((session.metadata?.pc_addresses ?? []).filter(address => address !== session.metadata?.pc_address));
  // On unless the settings file turns it off (`qr_pairing = false`).
  const qr = $derived(session.metadata ? session.metadata.qr_pairing !== false : false);
  const qrOff = $derived(session.metadata?.qr_pairing === false);
  const pairingOff = $derived(session.metadata?.enable_pairing === false);
  // The first device always gets Everything, so the choice waits for the second.
  const choosing = $derived((session.metadata?.paired_devices ?? 0) > 0 && !pairingOff);
  // Pairing is approved here, by showing a code or typing a PIN, so the device may play.
  let access = $state<Access>('play');
  let allowing = $state(false);

  // The pairing moment: a device that wasn't in the list the panel saw before.
  let own = $state<Client[] | null>(null);
  const list = $derived(clients === undefined ? own : clients);
  let known: Set<string> | null = null;
  let welcome = $state<Client | null>(null);
  $effect(() => {
    const current = list;
    if (!current) return;
    const added = newlyPaired(known, current);
    known = knownDevices(current);
    if (!added) return;
    welcome = added;
    untrack(() => {
      refreshMetadata().catch(() => {});
      onpaired?.(added);
    });
  });

  onMount(() => {
    refreshMetadata().catch(() => {});
    refreshPending().catch(() => {});
    if (clients !== undefined) return;
    return poll(async () => { own = (await api.clients.list()).clients; }, 5000);
  });

  // The PC's toast leads to the PIN field, its tray item to the code.
  let followed: string | null = null;
  $effect(() => {
    const target = pairLinkTarget(query('pair'), session.pending.length, qr);
    if (!target || followed === route.search) return;
    followed = route.search;
    void tick().then(() => {
      if (target === 'pin') document.querySelector<HTMLInputElement>('.requests input')?.focus();
      else document.getElementById('pair-qr')?.scrollIntoView();
    });
  });

  // The same keys and call as Allow pairing in Settings. Allowing pairing from the code's
  // place also brings the code back when the settings file turned it off.
  async function allow(values: { enable_pairing?: boolean; qr_pairing?: boolean }) {
    allowing = true;
    try {
      await api.config.patch(values);
      await refreshMetadata();
    } catch (error) {
      failed(values.enable_pairing ? 'Turning on pairing failed' : 'Turning on pairing with a QR code failed', error);
    } finally {
      allowing = false;
    }
  }
</script>

{#snippet unlock()}
  {#if pairingOff}
    <Button variant="primary" busy={allowing} onclick={() => allow(qrOff ? { enable_pairing: true, qr_pairing: true } : { enable_pairing: true })}>Allow pairing</Button>
  {:else}
    <Button variant="primary" busy={allowing} onclick={() => allow({ qr_pairing: true })}>Show a QR code</Button>
  {/if}
{/snippet}

{#snippet body()}
  <!-- The choice is for the next device, so it waits while the new one's card shows. -->
  {#if choosing && !welcome}
    <div class="access">
      <AccessChoice legend="This device can" recommend value={access} onchange={(next) => { if (next !== 'custom') access = next; }} />
    </div>
  {/if}
  <div class="ways">
    {#if welcome}
      <section class="way welcome" aria-labelledby="pair-welcome">
        <span class="mark" aria-hidden="true"><Icon name="check" size={28} /></span>
        <h3 id="pair-welcome">{welcome.name} is ready</h3>
        <p class="muted">Paired just now · Access: {accessLabel(welcome.perm)}</p>
        <div><Button onclick={() => (welcome = null)}>Pair another device</Button></div>
      </section>
    {:else}
      <section class="way" aria-labelledby="pair-qr">
        <h3 id="pair-qr"><Icon name="image" size={18} />Scan with Nightjar</h3>
        <QrPairing {access} lock={pairingOff || qrOff ? unlock : undefined}
          lockNote={!pairingOff ? 'Pairing with a QR code is off.' : (session.metadata?.paired_devices ?? 0) > 0 ? 'Devices already paired keep working.' : 'Pairing is off.'} />
      </section>
    {/if}
    <section class="way" aria-labelledby="pair-pin">
      <h3 id="pair-pin"><Icon name="key" size={18} />Moonlight and other apps</h3>
      {#if session.metadata}
        <div class="host-address"><Icon name="monitor" size={20} /><span class="who"><strong>{session.metadata.host_name}</strong><span class="num">{session.metadata.pc_address}</span></span><span class="copy"><CopyButton value={session.metadata.pc_address} label="Copy address" what="the address" /></span></div>
        {#if others.length}<details class="addresses"><summary>Other addresses</summary><p class="num">{others.join(', ')}</p></details>{/if}
      {:else}<p class="muted">Loading this PC's address…</p>{/if}
      {#if pairingOff}
        <p class="waiting muted">Pairing is turned off</p>
      {:else if session.pending.length}
        <ul class="requests" aria-label="Waiting for a PIN">{#each session.pending as request (request.uniqueid)}<PendingRequest {request} {access} />{/each}</ul>
      {:else}
        <p class="waiting muted"><span class="pulse" aria-hidden="true"></span>Waiting for a device. Its PIN request shows here.</p>
      {/if}
    </section>
  </div>
  <p class="visually-hidden" role="status">{welcome ? `${welcome.name} is ready.` : ''}</p>
  {#if !pairingOff}
    <details class="alternatives"><summary>Other ways to pair</summary><p class="muted">A one-time PIN for Artemis.</p><OtpPairing {access} /></details>
  {/if}
{/snippet}

{#if bare}
  {@render body()}
{:else}
  <Panel {title}>{@render body()}</Panel>
{/if}

<style>
  .access { display: grid; gap: var(--space-2); margin-bottom: var(--space-4); }
  .ways { display: grid; grid-template-columns: minmax(0, 1fr) minmax(0, 1fr); gap: var(--space-4); }
  .way { display: grid; gap: var(--space-3); align-content: start; padding: var(--space-4) var(--space-5); border: 1px solid var(--line); border-radius: var(--radius-lg); background: var(--panel); }
  .way h3 { display: flex; align-items: center; gap: var(--space-2); font-size: var(--text-md); font-weight: var(--weight-semibold); }
  .way h3 :global(svg) { color: var(--accent-text); }
  .welcome { justify-items: start; align-content: center; gap: var(--space-2); border-color: color-mix(in srgb, var(--ok) 45%, var(--line)); background: linear-gradient(160deg, var(--ok-soft), var(--panel) 70%); animation: welcome var(--duration-slow) var(--ease-enter); }
  .welcome h3 { font-family: var(--font-display); font-size: var(--text-lg); }
  .welcome > div { margin-top: var(--space-2); }
  .mark { display: grid; place-items: center; width: 52px; height: 52px; border-radius: 50%; background: var(--ok); color: var(--panel); }
  @keyframes welcome { from { opacity: 0; transform: scale(0.97); } }
  .host-address { display: flex; align-items: center; gap: var(--space-3); padding: var(--space-3) var(--space-4); background: var(--sunken); border: 1px solid var(--line); border-radius: var(--radius); }
  .host-address > :global(svg) { flex: none; }
  .who { display: flex; flex-wrap: wrap; align-items: baseline; gap: 0 var(--space-3); flex: 1; min-width: 0; overflow-wrap: anywhere; }
  .host-address strong { font-weight: 500; }
  .host-address .num { color: var(--ink-2); font-size: var(--text-sm); }
  .copy { display: inline-flex; flex: none; align-items: center; }
  .requests { display: grid; gap: var(--space-3); list-style: none; padding: 0; margin: 0; }
  .waiting { display: flex; align-items: center; gap: var(--space-3); padding: var(--space-4); background: var(--sunken); border: 1px dashed var(--line-strong); border-radius: var(--radius); font-size: var(--text-sm); }
  .pulse { flex: none; width: 8px; height: 8px; border-radius: 50%; background: var(--accent); animation: pulse var(--pulse-duration) ease-in-out infinite; }
  @keyframes pulse { 50% { opacity: 0.25; } }
  .alternatives { margin-top: var(--space-4); }
  .alternatives > p { margin: var(--space-3) 0; font-size: var(--text-sm); }
  .addresses { font-size: var(--text-sm); }
  @media (max-width: 900px) { .ways { grid-template-columns: minmax(0, 1fr); } }
  @media (max-width: 600px) { .way { padding: var(--space-4); } }
</style>
