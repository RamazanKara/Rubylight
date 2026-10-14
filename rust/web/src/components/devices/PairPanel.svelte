<script lang="ts">
  import { onMount } from 'svelte';
  import Badge from '../Badge.svelte';
  import CopyButton from '../CopyButton.svelte';
  import Panel from '../Panel.svelte';
  import OtpPairing from './OtpPairing.svelte';
  import PendingRequest from './PendingRequest.svelte';
  import { link } from '../../lib/router.svelte';
  import { refreshMetadata, refreshPending, session } from '../../lib/session.svelte';

  type Method = 'pin' | 'otp';
  const METHODS: { value: Method; label: string; hint: string }[] = [
    { value: 'pin', label: 'PIN from the device', hint: 'Moonlight and Artemis show a PIN; you type it here.' },
    { value: 'otp', label: 'One-time PIN', hint: 'Artemis only: make a PIN here and type it on the device.' },
  ];

  const uid = $props.id();
  let method = $state<Method>('pin');
  const tabs: Record<Method, HTMLButtonElement | undefined> = $state({ pin: undefined, otp: undefined });

  const others = $derived.by(() => {
    const metadata = session.metadata;
    if (!metadata) return [];
    return [...new Set(metadata.pc_addresses)].filter((address) => address && address !== metadata.pc_address);
  });
  const waiting = $derived(session.pending.length);

  // Arrow keys move between the tabs, as in any tab list.
  function keys(event: KeyboardEvent) {
    if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return;
    event.preventDefault();
    const index = METHODS.findIndex((item) => item.value === method);
    const step = event.key === 'ArrowRight' ? 1 : -1;
    const next =
      event.key === 'Home' ? 0 : event.key === 'End' ? METHODS.length - 1 : (index + step + METHODS.length) % METHODS.length;
    method = METHODS[next]?.value ?? method;
    tabs[method]?.focus();
  }

  // The shell refreshes requests every 3 s; do not wait for its next turn.
  onMount(() => {
    refreshMetadata().catch(() => {});
    refreshPending().catch(() => {});
  });
</script>

<Panel title="Pair a device">
  {#if session.metadata?.enable_pairing === false}
    <p class="notice">Pairing is disabled. Turn on Allow pairing in <a href="/settings/general" use:link>General settings</a> to pair a new device. Devices already paired keep working.</p>
  {:else}
    <div class="pair">
      <div class="tabs" role="tablist" aria-label="Pairing method" tabindex="-1" onkeydown={keys}>
        {#each METHODS as item (item.value)}
          <button
            bind:this={tabs[item.value]}
            type="button"
            role="tab"
            id="{uid}-{item.value}-tab"
            aria-selected={method === item.value}
            aria-controls="{uid}-{item.value}"
            tabindex={method === item.value ? 0 : -1}
            onclick={() => (method = item.value)}
          >
            <span class="tab-label">
              {item.label}
              {#if item.value === 'pin' && waiting}<Badge tone="accent">{waiting} waiting</Badge>{/if}
            </span>
            <span class="tab-hint">{item.hint}</span>
          </button>
        {/each}
      </div>

      <div id="{uid}-pin" role="tabpanel" aria-labelledby="{uid}-pin-tab" hidden={method !== 'pin'}>
        <ol class="steps">
          <li>
            <span class="step num" aria-hidden="true">1</span>
            <div class="step-body">
              <p>In Moonlight or Artemis, add this PC and select it.</p>
              {#if session.metadata}
                <dl class="facts">
                  <dt>Name</dt>
                  <dd>{session.metadata.host_name}</dd>
                  <dt>Address</dt>
                  <dd class="address">
                    <span class="num">{session.metadata.pc_address}</span>
                    <CopyButton value={session.metadata.pc_address} label="Copy address" what="the address" />
                  </dd>
                  {#if others.length}
                    <dt>Other addresses</dt>
                    <dd class="num">{others.join(', ')}</dd>
                  {/if}
                </dl>
              {:else}
                <p class="muted small">Loading this PC's address…</p>
              {/if}
            </div>
          </li>
          <li>
            <span class="step num" aria-hidden="true">2</span>
            <div class="step-body">
              <p>It shows a 4-digit PIN. Enter that PIN below.</p>
              {#if waiting}
                <ul class="requests" aria-label="Waiting for a PIN">
                  {#each session.pending as request (request.uniqueid)}
                    <PendingRequest {request} />
                  {/each}
                </ul>
              {:else}
                <p class="muted small" role="status">No device is waiting to pair. Start pairing on the device; it shows up here within a few seconds.</p>
              {/if}
            </div>
          </li>
        </ol>
      </div>

      <div id="{uid}-otp" role="tabpanel" aria-labelledby="{uid}-otp-tab" hidden={method !== 'otp'}>
        <OtpPairing />
      </div>
    </div>
  {/if}
</Panel>

<style>
  .pair {
    display: grid;
    gap: var(--space-5);
  }
  .tabs {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: var(--space-2);
    max-width: 720px;
  }
  [role='tab'] {
    display: grid;
    gap: 2px;
    align-content: start;
    padding: var(--space-3) var(--space-4);
    border: 1px solid var(--line);
    border-radius: var(--radius);
    background: var(--sunken);
    color: var(--ink);
    text-align: left;
    cursor: pointer;
  }
  [role='tab']:hover {
    border-color: var(--line-strong);
  }
  /* The chosen method reads as the card in front: panel colour and a ruby edge. */
  [role='tab'][aria-selected='true'] {
    border-color: var(--accent);
    background: var(--panel);
    box-shadow: inset 0 0 0 1px var(--accent);
  }
  .tab-label {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 4px var(--space-2);
    font-weight: 600;
    font-size: var(--text-sm);
  }
  .tab-hint {
    font-size: var(--text-xs);
    color: var(--ink-2);
    line-height: 1.45;
  }
  .steps {
    display: grid;
    gap: var(--space-4);
    margin: 0;
    padding: 0;
    list-style: none;
    max-width: 720px;
  }
  .steps > li {
    display: flex;
    gap: var(--space-3);
    align-items: flex-start;
  }
  .step {
    flex: none;
    display: grid;
    place-items: center;
    width: 22px;
    height: 22px;
    border: 1px solid var(--line-strong);
    border-radius: 50%;
    font-size: var(--text-xs);
    color: var(--ink-2);
  }
  .step-body {
    flex: 1;
    display: grid;
    gap: var(--space-2);
    min-width: 0;
    padding-top: 1px;
  }
  .facts {
    align-items: center;
  }
  .address {
    display: flex;
    align-items: center;
    gap: 2px;
  }
  .address .num {
    white-space: nowrap;
  }
  .requests {
    display: grid;
    gap: var(--space-2);
    margin: 0;
    padding: 0;
  }
  .small {
    font-size: var(--text-sm);
  }
  @media (max-width: 520px) {
    [role='tab'] {
      padding: var(--space-3);
    }
    /* Narrow labels wrap so an address keeps its line. */
    .facts {
      grid-template-columns: fit-content(6.5em) minmax(0, 1fr);
      gap: 6px var(--space-3);
    }
  }
</style>
