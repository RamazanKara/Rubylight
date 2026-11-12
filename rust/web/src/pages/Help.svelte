<script lang="ts">
  import Button from '../components/Button.svelte';
  import PageHeader from '../components/PageHeader.svelte';
  import ConnectionHelp from '../components/help/ConnectionHelp.svelte';
  import SupportBundlePanel from '../components/help/SupportBundlePanel.svelte';
  import LogAnalysisPanel from '../components/maintenance/LogAnalysisPanel.svelte';
  import Section from '../components/maintenance/Section.svelte';
  import { session } from '../lib/session.svelte';
</script>

<!-- Help (/help; /maintenance before): connecting, what the log says went wrong, and the support bundle.
     Restart, displays, the Windows session, co-op and crash reports are on Troubleshoot, under Advanced. -->
<div class="page">
  <PageHeader title="Help" subtitle="Connection, log findings and the support bundle.">
    {#snippet actions()}<Button href="/troubleshoot" icon="maintenance">Troubleshoot</Button>{/snippet}
  </PageHeader>
  <Section title="Connect a device" anchor="connect" quiet>
    <ConnectionHelp metadata={session.metadata} />
  </Section>
  <Section title="Log analysis" anchor="log-analysis" quiet>
    <LogAnalysisPanel />
  </Section>
  <Section title="Support bundle" anchor="support" quiet>
    <SupportBundlePanel />
  </Section>
</div>

<style>
  .page {
    display: grid;
    gap: var(--space-5);
    min-width: 0;
  }
</style>
