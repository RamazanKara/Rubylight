<script lang="ts">
  import Button from '../components/Button.svelte';
  import PageHeader from '../components/PageHeader.svelte';
  import ChildSessionPanel from '../components/maintenance/ChildSessionPanel.svelte';
  import CrashPanel from '../components/maintenance/CrashPanel.svelte';
  import LogAnalysisPanel from '../components/maintenance/LogAnalysisPanel.svelte';
  import DisplaysPanel from '../components/maintenance/DisplaysPanel.svelte';
  import HostPanel from '../components/maintenance/HostPanel.svelte';
  import NucleusPanel from '../components/maintenance/NucleusPanel.svelte';
  import Section from '../components/maintenance/Section.svelte';
  import WindowsSessionPanel from '../components/maintenance/WindowsSessionPanel.svelte';
  import { link } from '../lib/router.svelte';

  // Mounted from the start so it loads; its section shows once there is something to restore or fix.
  let childSession = $state(false);
</script>

<!-- Updates, power, signed-in browsers and the password live on the Settings home;
     the Vulkan HDR layer and RTSS under All settings › Display & HDR › Integrations. -->
<div class="page">
  <PageHeader title="Help" subtitle="Something is wrong? Start here.">
    {#snippet actions()}<Button href="/logs" icon="logs">View logs</Button>{/snippet}
  </PageHeader>
  <div id="host"></div>
  <Section title="Host">
    <HostPanel />
  </Section>
  <Section title="Displays and Windows session">
    <DisplaysPanel />
    <WindowsSessionPanel />
    <NucleusPanel />
  </Section>
  <Section title="Logs">
    <LogAnalysisPanel />
  </Section>
  <div class="optional" hidden={!childSession}>
    <Section title="Background child session">
      <ChildSessionPanel bind:shown={childSession} />
    </Section>
  </div>
  <Section title="Crashes and support bundle">
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
    gap: var(--space-6);
    min-width: 0;
  }
  .optional[hidden] {
    display: none;
  }
  .moved {
    font-size: var(--text-sm);
  }
</style>
