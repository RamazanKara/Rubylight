<script lang="ts">
  import Button from '../components/Button.svelte';
  import PageHeader from '../components/PageHeader.svelte';
  import ChildSessionBanner from '../components/maintenance/ChildSessionBanner.svelte';
  import CoopHelpPanel from '../components/maintenance/CoopHelpPanel.svelte';
  import BackupPanel from '../components/maintenance/BackupPanel.svelte';
  import CrashPanel from '../components/maintenance/CrashPanel.svelte';
  import DisplaysPanel from '../components/maintenance/DisplaysPanel.svelte';
  import HostPanel from '../components/maintenance/HostPanel.svelte';
  import Section from '../components/maintenance/Section.svelte';
  import WindowsSessionPanel from '../components/maintenance/WindowsSessionPanel.svelte';
</script>

<!-- Troubleshoot (/troubleshoot, under Advanced): putting Rubylight and the PC right again. Help has the
     log analysis and the support bundle; updates, wake, sleep and sign-in are in Settings, the Vulkan HDR
     layer and RTSS in All settings. -->
<div class="page">
  <PageHeader title="Troubleshoot" subtitle="Restart, displays, co-op, crash reports and settings backup.">
    {#snippet actions()}<Button href="/logs" icon="logs">View logs</Button>{/snippet}
  </PageHeader>
  <ChildSessionBanner />
  <Section title="Rubylight" anchor="host" quiet>
    <HostPanel />
  </Section>
  <Section title="Displays and Windows session" anchor="displays" quiet>
    <DisplaysPanel />
    <WindowsSessionPanel />
  </Section>
  <Section title="Co-op" anchor="coop" quiet>
    <CoopHelpPanel />
  </Section>
  <Section title="Crash reports" anchor="crashes" quiet>
    <CrashPanel />
  </Section>
  <Section title="Settings backup" anchor="backup" quiet>
    <BackupPanel />
  </Section>
</div>

<style>
  .page {
    display: grid;
    gap: var(--space-5);
    min-width: 0;
  }
</style>
