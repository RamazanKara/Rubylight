<script lang="ts">
  // Help's connection facts: what a streaming app lists this PC as, and the address to add it by.
  import Button from '../Button.svelte';
  import CopyButton from '../CopyButton.svelte';
  import Panel from '../Panel.svelte';
  import type { Metadata } from '../../lib/api';

  let { metadata }: { metadata: Metadata | null } = $props();

  const others = $derived(metadata ? (metadata.pc_addresses ?? []).filter((address) => address !== metadata.pc_address) : []);
  const paired = $derived(metadata?.paired_devices ?? null);
</script>

<Panel title="Connect a device" level={3} description="What a streaming app needs to find this PC.">
  {#snippet actions()}
    <Button href="/devices?pair=qr" icon="link" size="sm" variant={paired === 0 ? 'primary' : 'secondary'}>Pair a device</Button>
  {/snippet}
  {#if metadata}
    <dl class="facts">
      <dt>Listed as</dt>
      <dd><strong>{metadata.host_name}</strong></dd>
      <dt>Address</dt>
      <dd class="address">
        <span class="num">{metadata.pc_address}</span>
        <CopyButton value={metadata.pc_address} label="Copy address" what="the address" />
      </dd>
      {#if others.length}
        <dt>Other networks</dt>
        <dd class="num">{others.join(', ')}</dd>
      {/if}
      <dt>Paired devices</dt>
      <dd>{paired === 0 ? 'None yet' : paired}</dd>
    </dl>
  {:else}
    <p class="muted loading" aria-busy="true">Loading this PC’s address…</p>
  {/if}
</Panel>

<style>
  .facts {
    align-items: center;
  }
  strong {
    font-weight: var(--weight-semibold);
  }
  /* The copy button keeps its 44 px target but overlaps the rows around it. */
  .address {
    display: flex;
    align-items: center;
    gap: 2px;
    margin-block: -10px;
  }
  .loading {
    font-size: var(--text-sm);
  }
</style>
