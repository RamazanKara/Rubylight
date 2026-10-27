<script lang="ts">
  import Button from '../Button.svelte';
  import CopyButton from '../CopyButton.svelte';
  import type { Metadata } from '../../lib/api';

  let { metadata }: { metadata: Metadata | null } = $props();

  const others = $derived(metadata ? (metadata.pc_addresses ?? []).filter((address) => address !== metadata.pc_address) : []);
</script>

<div class="idle">
  <p class="lead">No device is streaming. To connect one:</p>
  <ol>
    <li>
      <span class="step num" aria-hidden="true">1</span>
      <span>Open Moonlight or Artemis on the device{#if metadata}{' '}and select <strong>{metadata.host_name}</strong>{/if}.</span>
    </li>
    <li>
      <span class="step num" aria-hidden="true">2</span>
      <span>
        If this PC isn't listed, add it by address
        {#if metadata}
          <span class="address"><code>{metadata.pc_address}</code><CopyButton value={metadata.pc_address} label="Copy address" what="the address" /></span>
          {#if others.length}
            <span class="others">
              On another network:
              {#each others as address, index}{index ? ', ' : ''}<code>{address}</code>{/each}
            </span>
          {/if}
        {:else}
          <span class="muted">(loading…)</span>
        {/if}
      </span>
    </li>
    <li>
      <span class="step num" aria-hidden="true">3</span>
      <span>Pair with the PIN the device shows, then choose an app.</span>
    </li>
  </ol>
  <div>
    <Button href="/devices" icon="link" size="sm" variant={metadata?.paired_devices === 0 ? 'primary' : 'secondary'}>
      Pair a device
    </Button>
  </div>
</div>

<style>
  .idle {
    display: grid;
    gap: var(--space-3);
    padding: var(--space-4) var(--space-5) var(--space-5);
    font-size: var(--text-sm);
  }
  .lead {
    color: var(--muted);
  }
  ol {
    display: grid;
    gap: var(--space-2);
    margin: 0;
    padding: 0;
    list-style: none;
    color: var(--ink-2);
  }
  li {
    display: flex;
    gap: var(--space-3);
    align-items: flex-start;
  }
  li > span:last-child {
    min-width: 0;
    padding-top: 1px;
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
  strong {
    font-weight: var(--weight-semibold);
    color: var(--ink);
  }
  code {
    color: var(--ink);
    overflow-wrap: anywhere;
  }
  .address {
    display: inline-flex;
    align-items: center;
    gap: 2px;
    vertical-align: middle;
  }
  .address code {
    padding: 1px 6px;
    border: 1px solid var(--line);
    border-radius: var(--radius);
    background: var(--sunken);
  }
  .others {
    display: block;
    margin-top: 2px;
    font-size: var(--text-xs);
    color: var(--muted);
  }
  @media (max-width: 520px) {
    .idle {
      padding: var(--space-4);
    }
  }
</style>
