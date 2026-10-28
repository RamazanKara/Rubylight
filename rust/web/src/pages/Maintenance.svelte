<script lang="ts">
  import Button from '../components/Button.svelte';
  import PageHeader from '../components/PageHeader.svelte';
  import ChildSessionPanel from '../components/maintenance/ChildSessionPanel.svelte';
  import CrashPanel from '../components/maintenance/CrashPanel.svelte';
  import LogAnalysisPanel from '../components/maintenance/LogAnalysisPanel.svelte';
  import DisplaysPanel from '../components/maintenance/DisplaysPanel.svelte';
  import HostPanel from '../components/maintenance/HostPanel.svelte';
  import LimiterPanel from '../components/maintenance/LimiterPanel.svelte';
  import Section from '../components/maintenance/Section.svelte';
  import VulkanPanel from '../components/maintenance/VulkanPanel.svelte';
  import WindowsSessionPanel from '../components/maintenance/WindowsSessionPanel.svelte';
  import { link } from '../lib/router.svelte';
</script>

<!-- Updates, power, signed-in browsers and the password live on the Settings home. -->
<div class="page">
  <PageHeader title="Help & diagnostics" subtitle="Check logs, recover your displays, or restart the host.">
    {#snippet actions()}<Button href="/logs" icon="logs">View logs</Button>{/snippet}
  </PageHeader>
  <div id="host"></div>
  <Section title="Host">
    <HostPanel />
  </Section>
  <Section title="Displays and Windows session">
    <DisplaysPanel />
    <WindowsSessionPanel />
    <ChildSessionPanel />
  </Section>
  <Section title="Troubleshooting">
    <LogAnalysisPanel />
    <CrashPanel />
  </Section>
  <Section title="Integrations" columns={2}>
    <VulkanPanel />
    <LimiterPanel />
  </Section>
  <p class="moved muted">
    Updates, wake and sleep, signed-in browsers and the password are in <a href="/settings" use:link>Settings</a>.
  </p>
</div>

<style>
  .page {
    display: grid;
    gap: var(--space-6);
    min-width: 0;
  }
  .moved {
    font-size: var(--text-sm);
  }
</style>
