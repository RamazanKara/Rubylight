<script lang="ts">
  // The pairing QR code, shown as soon as the page opens: a one-time PIN and passphrase that
  // work once for three minutes, with this PC's certificate fingerprint. A new code is made
  // only when asked, so a one-time PIN made for Artemis is not replaced behind your back.
  import { onMount } from 'svelte';
  import Button from '../Button.svelte';
  import CopyButton from '../CopyButton.svelte';
  import { api, type PairingCode } from '../../lib/api';

  const uid = $props.id();
  let codes = $state<PairingCode[]>([]);
  let chosen = $state('');
  let expires = $state(0);
  let now = $state(Date.now());
  let busy = $state(false);
  let error = $state('');

  const code = $derived(codes.find(c => c.address === chosen) ?? codes[0]);
  const left = $derived(code ? Math.max(0, Math.ceil((expires - now) / 1000)) : 0);
  const expired = $derived(Boolean(code) && left === 0);
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
    error = '';
    try {
      const result = await api.clients.pairingQr(chosen);
      now = Date.now();
      codes = result.codes;
      chosen = result.codes[0]?.address ?? '';
      expires = now + result.expires_in_seconds * 1000;
    } catch (failure) {
      error = failure instanceof Error ? failure.message : String(failure);
    } finally {
      busy = false;
    }
  }
  onMount(() => {
    void create();
  });
</script>

<div class="qr">
  <div class="frame" class:expired aria-busy={busy && !code}>
    {#if code}
      <img src={image} alt="Pairing QR code for {code.address}" width="200" height="200" />
    {:else}
      <span class="blank" aria-hidden="true"></span>
    {/if}
    {#if expired}
      <div class="again"><Button variant="primary" size="sm" {busy} onclick={create}>Show a new code</Button></div>
    {/if}
  </div>
  <div class="details" aria-live="polite">
    {#if error}
      <p class="error" role="alert">The QR code could not be made: {error}</p>
      <div><Button size="sm" {busy} onclick={create}>Try again</Button></div>
    {:else if !code}
      <p class="muted">Making a code…</p>
    {:else if expired}
      <p><strong>This code expired.</strong> Codes last three minutes and work once.</p>
    {:else}
      <p class="muted" aria-live="off">Works once, for <span class="num">{countdown}</span>.</p>
      <div><CopyButton value={code.uri} label="Copy pairing link" what="the pairing link" showLabel /></div>
    {/if}
    {#if codes.length > 1}
      <label for="{uid}-address">Address your device reaches</label>
      <select id="{uid}-address" class="input" bind:value={chosen}>
        {#each codes as option (option.address)}<option value={option.address}>{option.address}</option>{/each}
      </select>
    {/if}
  </div>
</div>

<style>
  .qr {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-4);
    align-items: center;
  }
  .frame {
    position: relative;
    flex: none;
    width: 216px;
    height: 216px;
    padding: var(--space-1);
    border: 1px solid var(--line-strong);
    border-radius: var(--radius);
    background: #fff;
  }
  img,
  .blank {
    display: block;
    width: 200px;
    height: 200px;
    image-rendering: pixelated;
  }
  .blank {
    border-radius: var(--radius-sm);
    background: var(--skeleton);
  }
  .expired img {
    opacity: 0.12;
  }
  .again {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
  }
  .details {
    display: grid;
    gap: var(--space-2);
    flex: 1 1 180px;
    font-size: var(--text-sm);
  }
  .error {
    color: var(--danger);
  }
</style>
