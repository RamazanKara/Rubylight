<script lang="ts">
  import Icon from '../Icon.svelte';
  import type { Client } from '../../lib/api';

  let { client }: { client: Pick<Client, 'client_agent' | 'client_warning'> } = $props();
</script>

{#if client.client_warning}
  <p class="notice warn client-warning" role="status">
    <span>This client build has a known problem: {client.client_warning.problem}. It can still connect.</span>
    <a href={client.client_warning.link} target="_blank" rel="noreferrer">Details<Icon name="external" size={14} /></a>
  </p>
{/if}
{#if client.client_agent}
  <p class="agent muted">Client: <span class="mono">{client.client_agent}</span></p>
{/if}

<style>
  .agent,
  .client-warning {
    margin: var(--space-1) 0 0;
    font-size: var(--text-sm);
    max-width: 65ch;
    overflow-wrap: anywhere;
  }
  a {
    display: inline-flex;
    align-items: center;
    gap: 4px;
  }
</style>
