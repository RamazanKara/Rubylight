<script lang="ts">
  import { onMount } from 'svelte';
  import Button from '../Button.svelte';
  import Panel from '../Panel.svelte';
  import LoadState from '../LoadState.svelte';
  import LogFindings from './LogFindings.svelte';
  import { api, type LogAnalysis } from '../../lib/api';
  import { reason } from './util';

  let analysis = $state<LogAnalysis | null>(null);
  let loadError = $state('');
  let busy = $state(false);

  async function load() {
    busy = true;
    try {
      analysis = await api.logs.analysis();
      loadError = '';
    } catch (error) {
      loadError = reason(error);
    } finally {
      busy = false;
    }
  }

  onMount(load);
</script>

<Panel title="Log analysis" level={3} description="Known failures in the latest host log, explained on this PC. Nothing is sent anywhere.">
  <div class="stack">
    {#if analysis && loadError}
      <LoadState error={loadError} what="the log analysis" stale onretry={load} />
    {/if}
    {#if !analysis}
      <LoadState error={loadError} what="the log analysis" onretry={load} />
    {:else}
      <LogFindings {analysis} />
    {/if}
    <div class="row">
      <Button {busy} icon="refresh" onclick={load}>Analyze again</Button>
      <Button href="/logs" variant="ghost" icon="logs">Open logs</Button>
    </div>
  </div>
</Panel>
