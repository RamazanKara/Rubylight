<script lang="ts">
  import { onMount } from 'svelte';
  import Badge from '../Badge.svelte';
  import Panel from '../Panel.svelte';
  import LoadState from '../LoadState.svelte';
  import { api, type LimiterStatus } from '../../lib/api';
  import { reason } from './util';

  let limiter = $state<LimiterStatus | null>(null);
  let loadError = $state('');

  async function load() {
    try {
      limiter = await api.health.limiter();
      loadError = '';
    } catch (error) {
      loadError = reason(error);
    }
  }

  onMount(load);

  const providers: Record<string, string> = {
    auto: 'Automatic',
    rtss: 'RTSS',
    none: 'None',
  };
  const provider = (name: string) => providers[name] ?? (name ? 'Other limiter' : 'None');
</script>

<Panel title="Frame limiter" level={3} description="Caps game frame rates during streams through RTSS.">
  {#snippet actions()}
    {#if limiter}<Badge tone={limiter.enabled ? 'ok' : 'neutral'}>{limiter.enabled ? 'On' : 'Off'}</Badge>{/if}
  {/snippet}
  {#if limiter}
    <div class="stack">
      <dl class="facts">
        <dt>Configured</dt>
        <dd>{provider(limiter.configured_provider)}</dd>
        <dt>Active now</dt>
        <dd>{provider(limiter.active_provider)}</dd>
        <dt>RTSS</dt>
        <dd>
          {#if limiter.rtss_available}
            Found{limiter.process_running ? ', running' : ', not running'}
          {:else if limiter.path_exists}
            Folder found, but RTSS is incomplete
          {:else}
            Not found
          {/if}
        </dd>
        <dt>RTSS folder</dt>
        <dd class="mono">{limiter.resolved_path || '—'}</dd>
      </dl>
      {#if limiter.message}<p class="muted note">{limiter.message}</p>{/if}
    </div>
  {:else}
    <LoadState error={loadError} what="the frame limiter status" onretry={load} />
  {/if}
</Panel>

<style>
  .note {
    font-size: var(--text-sm);
  }
</style>
