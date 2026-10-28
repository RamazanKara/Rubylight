<script lang="ts">
  import Button from '../Button.svelte';
  import CopyButton from '../CopyButton.svelte';
  import { api, type PairingCode } from '../../lib/api';
  import { failed } from '../../lib/feedback.svelte';

  const uid = $props.id();
  let codes = $state<PairingCode[]>([]);
  let chosen = $state('');
  let expires = $state(0);
  let now = $state(Date.now());
  let busy = $state(false);

  const code = $derived(codes.find(c => c.address === chosen) ?? codes[0]);
  const left = $derived(code ? Math.max(0, Math.ceil((expires - now) / 1000)) : 0);
  const expired = $derived(left === 0);
  const countdown = $derived(`${Math.floor(left / 60)}:${String(left % 60).padStart(2, '0')}`);
  // The host renders the code; showing it as an image keeps its markup inert.
  const image = $derived(code ? `data:image/svg+xml;charset=utf-8,${encodeURIComponent(code.svg)}` : '');

  $effect(() => {
    if (!code || expired) return;
    const timer = setInterval(() => (now = Date.now()), 1000);
    return () => clearInterval(timer);
  });

  async function create() {
    busy = true;
    try {
      const result = await api.clients.pairingQr(chosen);
      now = Date.now();
      codes = result.codes;
      chosen = result.codes[0]?.address ?? '';
      expires = now + result.expires_in_seconds * 1000;
    } catch (error) {
      failed('Creating a pairing code failed', error);
    } finally {
      busy = false;
    }
  }
</script>

<div class="qr">
  <p class="muted">Nightjar on Android: tap Scan QR code when adding a PC and point the camera at this code. Nightjar on Windows and Linux: copy the link and paste it in Add a PC. Experimental.</p>
  {#if code}
    <div class="result" class:expired aria-live="polite">
      <img src={image} alt="Pairing QR code for {code.address}" width="220" height="220" />
      <div class="details">
        {#if expired}
          <p><strong>This code expired.</strong> Create a new one.</p>
        {:else}
          <p aria-live="off">Expires in <span class="num">{countdown}</span>. Works once.</p>
          <div><CopyButton value={code.uri} label="Copy pairing link" what="the pairing link" showLabel /></div>
        {/if}
        {#if codes.length > 1}
          <label for="{uid}-address">Address the device reaches</label>
          <select id="{uid}-address" class="input" bind:value={chosen}>
            {#each codes as option (option.address)}<option value={option.address}>{option.address}</option>{/each}
          </select>
        {:else}
          <p class="muted num">{code.address}</p>
        {/if}
      </div>
    </div>
  {/if}
  <div>
    <Button variant="primary" {busy} onclick={create}>{code ? 'Create a new code' : 'Show QR code'}</Button>
  </div>
</div>

<style>
  .qr {
    display: grid;
    gap: var(--space-4);
    align-content: start;
    max-width: 720px;
  }
  .qr > p {
    font-size: var(--text-sm);
  }
  .result {
    display: flex;
    gap: var(--space-3) var(--space-4);
    align-items: center;
    flex-wrap: wrap;
    padding: var(--space-3) var(--space-4);
    border: 1px solid var(--line-strong);
    border-radius: var(--radius);
    background: var(--sunken);
  }
  img {
    display: block;
    border-radius: var(--radius);
    background: #fff;
    image-rendering: pixelated;
  }
  .expired img {
    opacity: 0.25;
  }
  .details {
    display: grid;
    gap: var(--space-2);
    flex: 1 1 200px;
    font-size: var(--text-sm);
  }
</style>
