<script lang="ts">
  import Badge from '../Badge.svelte';
  import Button from '../Button.svelte';
  import Panel from '../Panel.svelte';
  import type { UsbAttachment } from '../../lib/api';
  import { byClient, usbId, usbState } from './usb';

  let {
    devices,
    error = '',
    busy = null,
    ondetach,
  }: {
    devices: UsbAttachment[];
    /** Why the list could not be loaded. */
    error?: string;
    /** The attachment being detached. */
    busy?: number | null;
    ondetach: (device: UsbAttachment) => void;
  } = $props();

  const groups = $derived(byClient(devices));
</script>

<Panel
  title="USB devices (Experimental)"
  description="USB devices your paired devices attach to this PC during their streams. Each device needs USB devices in its permissions."
  flush
>
  {#if error}
    <p class="notice danger usb-error" role="alert">Could not load USB devices: {error}</p>
  {/if}
  {#if groups.length === 0}
    <p class="placeholder muted">No USB devices are attached.</p>
  {:else}
    {#each groups as group (group.client)}
      <section class="client">
        <h3>{group.client}</h3>
        <ul>
          {#each group.devices as device (device.id)}
            {@const state = usbState(device)}
            <li>
              <div class="identity">
                <span class="name">{device.name}</span>
                <span class="muted mono">{usbId(device)} · {device.busid}</span>
                {#if device.error}<span class="detach-error">{device.error}</span>{/if}
              </div>
              <Badge tone={state.tone}>{state.label}</Badge>
              <Button size="sm" disabled={busy !== null || device.state === 'detaching'} busy={busy === device.id} onclick={() => ondetach(device)}>
                {device.state === 'detach_failed' ? 'Detach again' : 'Detach'}<span class="visually-hidden"> {device.name}</span>
              </Button>
            </li>
          {/each}
        </ul>
      </section>
    {/each}
  {/if}
</Panel>

<style>
  .placeholder {
    padding: var(--space-5);
    font-size: var(--text-sm);
  }
  .usb-error {
    margin: var(--space-3) var(--space-5);
  }
  .client {
    padding: var(--space-3) var(--space-5);
  }
  .client + .client {
    border-top: 1px solid var(--line);
  }
  h3 {
    margin: 0 0 var(--space-2);
    font-size: var(--text-sm);
    font-weight: var(--weight-semibold);
  }
  ul {
    display: grid;
    gap: var(--space-2);
    margin: 0;
    padding: 0;
    list-style: none;
  }
  li {
    display: flex;
    align-items: center;
    gap: var(--space-2) var(--space-3);
    flex-wrap: wrap;
  }
  .identity {
    display: grid;
    gap: 2px;
    flex: 1 1 220px;
    min-width: 0;
    font-size: var(--text-sm);
  }
  .name {
    font-weight: var(--weight-semibold);
    overflow-wrap: anywhere;
  }
  .detach-error {
    color: var(--danger);
    overflow-wrap: anywhere;
  }
</style>
