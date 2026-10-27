<script lang="ts">
  import Button from '../components/Button.svelte';
  import { onMount } from 'svelte';
  import PageHeader from '../components/PageHeader.svelte';
  import CrashPanel from '../components/maintenance/CrashPanel.svelte';
  import LogAnalysisPanel from '../components/maintenance/LogAnalysisPanel.svelte';
  import DisplaysPanel from '../components/maintenance/DisplaysPanel.svelte';
  import HostPanel from '../components/maintenance/HostPanel.svelte';
  import LimiterPanel from '../components/maintenance/LimiterPanel.svelte';
  import PasswordPanel from '../components/maintenance/PasswordPanel.svelte';
  import PowerPanel from '../components/maintenance/PowerPanel.svelte';
  import Section from '../components/maintenance/Section.svelte';
  import SessionsPanel from '../components/maintenance/SessionsPanel.svelte';
  import UpdatesPanel from '../components/maintenance/UpdatesPanel.svelte';
  import VulkanPanel from '../components/maintenance/VulkanPanel.svelte';
  import WindowsSessionPanel from '../components/maintenance/WindowsSessionPanel.svelte';
  import { reason } from '../components/maintenance/util';
  import { api, type WebSession } from '../lib/api';

  // Shared by the browser list and the password form, which fills in the username.
  let sessions = $state<WebSession[] | null>(null);
  let sessionsError = $state('');
  const username = $derived(sessions?.find((session) => session.current)?.username ?? '');

  async function loadSessions() {
    try {
      sessions = (await api.auth.sessions()).sessions;
      sessionsError = '';
    } catch (error) {
      sessionsError = reason(error);
    }
  }

  onMount(loadSessions);
</script>

<div class="page">
  <PageHeader title="Help & diagnostics" subtitle="Check logs, recover your displays, or manage this host.">
    {#snippet actions()}<Button href="/logs" icon="logs">View logs</Button>{/snippet}
  </PageHeader>
  <div id="host"></div>
  <Section title="Host">
    <HostPanel />
    <UpdatesPanel />
  </Section>
  <Section title="Power">
    <PowerPanel />
  </Section>
  <Section title="Displays and Windows session">
    <DisplaysPanel />
    <WindowsSessionPanel />
  </Section>
  <Section title="Troubleshooting">
    <LogAnalysisPanel />
    <CrashPanel />
  </Section>
  <Section title="Integrations" columns={2}>
    <VulkanPanel />
    <LimiterPanel />
  </Section>
  <div id="account"></div>
  <Section title="Security" columns={2}>
    <SessionsPanel {sessions} error={sessionsError} onchange={loadSessions} />
    <PasswordPanel {username} />
  </Section>
</div>

<style>
  .page {
    display: grid;
    gap: var(--space-6);
    min-width: 0;
  }
</style>
