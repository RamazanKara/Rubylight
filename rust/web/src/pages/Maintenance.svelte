<script lang="ts">
  import Button from '../components/Button.svelte';
  import PageHeader from '../components/PageHeader.svelte';
  import ChildSessionBanner from '../components/maintenance/ChildSessionBanner.svelte';
  import CoopHelpPanel from '../components/maintenance/CoopHelpPanel.svelte';
  import CrashPanel from '../components/maintenance/CrashPanel.svelte';
  import LogAnalysisPanel from '../components/maintenance/LogAnalysisPanel.svelte';
  import DisplaysPanel from '../components/maintenance/DisplaysPanel.svelte';
  import HostPanel from '../components/maintenance/HostPanel.svelte';
  import Section from '../components/maintenance/Section.svelte';
  import WindowsSessionPanel from '../components/maintenance/WindowsSessionPanel.svelte';
  import { link } from '../lib/router.svelte';
</script>

<!-- Updates, power, signed-in browsers and the password live on the Settings home;
     the Vulkan HDR layer and RTSS under All settings › Display & HDR › Integrations. -->
<div class="page">
  <PageHeader title="Help" subtitle="Something is wrong? Start here.">
    {#snippet actions()}<Button href="/logs" icon="logs">View logs</Button>{/snippet}
  </PageHeader>
  <ChildSessionBanner />
  <div id="host"></div>
  <Section title="Host" quiet>
    <HostPanel />
  </Section>
  <Section title="Displays and Windows session" quiet>
    <DisplaysPanel />
    <WindowsSessionPanel />
  </Section>
  <Section title="Co-op" quiet>
    <CoopHelpPanel />
  </Section>
  <Section title="Logs" quiet>
    <LogAnalysisPanel />
  </Section>
  <Section title="Crashes and support bundle" quiet>
    <CrashPanel />
  </Section>
  <p class="moved muted">
    Updates, wake and sleep, signed-in browsers and the password are in <a href="/settings" use:link>Settings</a>. The Vulkan
    HDR layer and RTSS are in <a href="/settings/display" use:link>Display &amp; HDR</a>.
  </p>
</div>

<style>
  .page {
    display: grid;
    gap: var(--space-5);
    min-width: 0;
  }
  .moved {
    font-size: var(--text-sm);
  }
</style>
