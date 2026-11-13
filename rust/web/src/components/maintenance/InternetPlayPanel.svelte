<script lang="ts">
  import { onMount, untrack } from 'svelte';
  import Badge from '../Badge.svelte';
  import Panel from '../Panel.svelte';
  import LoadState from '../LoadState.svelte';
  import { api, type InternetPlayStatus } from '../../lib/api';
  import { reason } from './util';

  let { initial = null }: { initial?: InternetPlayStatus | null } = $props();

  let status = $state<InternetPlayStatus | null>(untrack(() => initial));
  let loadError = $state('');

  async function load() {
    try {
      status = await api.health.internetPlay();
      loadError = '';
    } catch (error) {
      loadError = reason(error);
    }
  }

  onMount(() => {
    if (!initial) load();
  });

  const natWords: Record<string, string> = {
    none: 'None: the PC has a public address',
    cone: 'Open to direct paths',
    symmetric: 'Changes ports per destination (symmetric)',
    unknown: 'Not known yet',
  };

  function ports(list: { protocol: string; port: number }[]): string {
    return list.map((entry) => `${entry.protocol} ${entry.port}`).join(', ');
  }
</script>

<Panel
  title="Internet status"
  level={3}
  description="What the router mapped and how devices outside your network reach this PC."
>
  {#if status}
    <div class="stack">
      <dl class="facts">
        <dt>Port forwarding</dt>
        <dd>
          {#if !status.mapping.enabled}
            <Badge tone="neutral">Off</Badge>
          {:else if status.mapping.error}
            <Badge tone="warn">Failed</Badge> <span class="muted">{status.mapping.error}</span>
          {:else if status.mapping.method && status.mapping.failed.length === 0}
            <Badge tone="ok">Mapped with {status.mapping.method}</Badge>
          {:else if status.mapping.method}
            <Badge tone="warn">Partly mapped with {status.mapping.method}</Badge>
          {:else}
            <Badge tone="neutral">Looking for the router</Badge>
          {/if}
        </dd>
        {#if status.mapping.mapped.length > 0}
          <dt>Mapped ports</dt>
          <dd>{ports(status.mapping.mapped)}</dd>
        {/if}
        {#each status.mapping.failed.filter((entry) => entry.reason && !status?.mapping.error) as entry (entry.protocol + entry.port)}
          <dt>{entry.protocol} {entry.port}</dt>
          <dd><Badge tone="warn">Not mapped</Badge> <span class="muted">{entry.reason}</span></dd>
        {/each}
        {#if status.mapping.ipv6Method || status.mapping.ipv6Error}
          <dt>IPv6 firewall</dt>
          <dd>
            {#if status.mapping.ipv6Pinholes > 0}
              <Badge tone="ok">{status.mapping.ipv6Pinholes} ports open with {status.mapping.ipv6Method}</Badge>
            {:else}
              <Badge tone="warn">Closed</Badge> <span class="muted">{status.mapping.ipv6Error}</span>
            {/if}
          </dd>
        {/if}
        <dt>Public address</dt>
        <dd>{status.externalIp ?? 'Not known'}</dd>
        <dt>Play over the internet</dt>
        <dd>
          {#if !status.enabled}
            <Badge tone="neutral">Off</Badge>
          {:else if !status.rendezvous}
            <Badge tone="warn">Not running</Badge> <span class="muted">Restart the host to start it.</span>
          {:else if status.rendezvous.stunError || status.rendezvous.dhtError}
            <Badge tone="warn">Not reachable</Badge>
            <span class="muted">{status.rendezvous.stunError ?? status.rendezvous.dhtError}</span>
          {:else}
            <Badge tone="ok">Reachable</Badge>
          {/if}
        </dd>
        {#if status.rendezvous}
          <dt>Router type</dt>
          <dd>{natWords[status.rendezvous.nat] ?? status.rendezvous.nat}</dd>
          <dt>Devices</dt>
          <dd>
            {status.rendezvous.published} of {status.rendezvous.devices} can find this PC
            {#if status.rendezvous.devices === 0}
              <span class="muted">(a device learns the way while it is on your network)</span>
            {/if}
          </dd>
          {#each status.rendezvous.paths as path (path.device)}
            <dt>{path.device || 'A device'}</dt>
            <dd>
              {#if path.connected}
                <Badge tone="ok">Connected directly</Badge> <span class="muted">{path.peer}</span>
              {:else}
                <Badge tone="neutral">Connecting</Badge>
              {/if}
            </dd>
          {/each}
        {/if}
      </dl>
      {#each status.notes as note (note)}
        <p class="muted note">{note}</p>
      {/each}
    </div>
  {:else}
    <LoadState error={loadError} what="the internet status" onretry={load} />
  {/if}
</Panel>

<style>
  .note {
    font-size: var(--text-xs);
    margin: 0;
  }
</style>
