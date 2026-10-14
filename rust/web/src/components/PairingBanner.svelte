<script lang="ts">
  import { api, type PendingPairing } from '../lib/api';
  import { failed, notify } from '../lib/feedback.svelte';
  import { refreshPending } from '../lib/session.svelte';
  import Button from './Button.svelte';
  import Icon from './Icon.svelte';

  let { requests }: { requests: PendingPairing[] } = $props();

  const first = $derived(requests[0]);
  let pin = $state('');
  let busy = $state(false);
  // The PIN belongs to the device it was typed for. When that request goes
  // away (paired elsewhere, expired), the next device must not receive it.
  let pinFor = '';
  $effect(() => {
    const id = first?.uniqueid ?? '';
    if (id !== pinFor) {
      pinFor = id;
      pin = '';
    }
  });

  function typed(event: Event & { currentTarget: HTMLInputElement }) {
    pin = event.currentTarget.value.replace(/\D/g, '').slice(0, 4);
    event.currentTarget.value = pin;
  }

  async function pair(event: SubmitEvent) {
    event.preventDefault();
    const target = first;
    if (!target || !/^\d{4}$/.test(pin)) return;
    busy = true;
    try {
      await api.clients.pin(pin, target.uniqueid, target.name);
      notify(`PIN sent to ${target.name}. Check the device for the result. If it rejects the PIN, start pairing again on the device and enter its new PIN.`, 'info', 8000);
      pin = '';
    } catch (error) {
      failed('Pairing failed', error);
    } finally {
      busy = false;
    }
    // Also after a failure: the request may have expired, and it should not stay on screen.
    await refreshPending().catch(() => {});
  }
</script>

{#if first}
  <form class="banner" onsubmit={pair}>
    <Icon name="link" />
    <div class="text">
      <strong>{first.name} wants to pair.</strong>
      <span class="muted">Enter the PIN shown on that device.{requests.length > 1
          ? ` ${requests.length - 1} more waiting.`
          : ''}</span>
    </div>
    <input
      class="input pin num"
      inputmode="numeric"
      autocomplete="one-time-code"
      placeholder="PIN"
      aria-label="PIN for {first.name}"
      value={pin}
      oninput={typed}
    />
    <Button type="submit" variant="primary" {busy} disabled={!/^\d{4}$/.test(pin)}>Pair</Button>
  </form>
{/if}

<style>
  .banner {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    padding: var(--space-3) var(--space-4);
    border-radius: var(--radius-lg);
    background: var(--accent-soft);
    border: 1px solid var(--accent);
    flex-wrap: wrap;
  }
  .text {
    display: grid;
    flex: 1;
    min-width: 220px;
    font-size: var(--text-sm);
  }
  .pin {
    width: 92px;
    text-align: center;
    letter-spacing: 0.3em;
    font-size: var(--text-lg);
  }
</style>
