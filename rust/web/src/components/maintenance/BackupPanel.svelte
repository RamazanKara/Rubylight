<script lang="ts">
  import { onDestroy } from 'svelte';
  import Banner from '../Banner.svelte';
  import Button from '../Button.svelte';
  import Panel from '../Panel.svelte';
  import Toggle from '../Toggle.svelte';
  import { api, type BackupInspection } from '../../lib/api';
  import { confirm, failed, notify } from '../../lib/feedback.svelte';
  import { backupFileName, changeLabel, fileProblem, restoreNote } from './backup';
  import { backAfterRestart } from './host';
  import { reason } from './util';

  let secrets = $state(false);
  let exporting = $state(false);
  let checking = $state(false);
  let restoring = $state(false);
  let late = $state(false);
  let problem = $state('');
  let inspection = $state<BackupInspection | null>(null);
  let fileInput = $state<HTMLInputElement>();
  let alive = true;
  onDestroy(() => (alive = false));

  async function download() {
    if (secrets) {
      const ok = await confirm({
        title: 'Include secrets?',
        message:
          'The file will hold this PC’s pairing identity, the paired devices’ certificates, the console sign-in, API tokens and webhook secrets. Anyone with the file can pose as this PC to your devices. Keep it somewhere private.',
        confirm: 'Download with secrets',
        danger: true,
      });
      if (!ok) return;
    }
    exporting = true;
    try {
      const blob = await api.backup.export(secrets);
      const url = URL.createObjectURL(blob);
      const link = document.createElement('a');
      link.href = url;
      link.download = backupFileName(secrets);
      document.body.append(link);
      link.click();
      link.remove();
      setTimeout(() => URL.revokeObjectURL(url), 10_000);
      notify('Settings backup downloaded.', 'ok');
    } catch (error) {
      failed('The backup failed', error);
    } finally {
      exporting = false;
    }
  }

  async function check(event: Event) {
    const input = event.currentTarget as HTMLInputElement;
    const file = input.files?.[0];
    input.value = '';
    if (!file) return;
    inspection = null;
    problem = fileProblem(file);
    if (problem) return;
    checking = true;
    try {
      inspection = await api.backup.inspect(file);
    } catch (error) {
      problem = reason(error);
    } finally {
      checking = false;
    }
  }

  async function restore() {
    if (!inspection) return;
    const ok = await confirm({
      title: 'Restore these settings?',
      message: restoreNote(inspection),
      confirm: 'Restore and restart',
      danger: inspection.summary.secrets,
    });
    if (!ok) return;
    restoring = true;
    try {
      const answer = await api.backup.restore(inspection.token);
      notify(`Current settings saved as ${answer.saved ?? 'a backup'}. Restarting.`, 'ok');
    } catch (error) {
      restoring = false;
      failed('The restore failed', error);
      return;
    }
    const back = await backAfterRestart(() => alive);
    if (!alive) return;
    restoring = false;
    inspection = null;
    if (back) notify('Settings restored. Rubylight restarted.', 'ok');
    else late = true;
  }
</script>

<!-- Troubleshoot's settings backup (/troubleshoot#backup); the tray's Restore settings opens it. -->
<Panel title="Settings backup" level={3}>
  <div class="stack">
    <p class="muted note">
      One file with the settings, apps, covers, each device’s own settings, game settings files and webhooks. Restore it here on this PC or another one.
    </p>
    <Toggle
      bind:checked={secrets}
      label="Include pairing identity and sign-in"
      hint="Secrets: this PC’s pairing certificate and key, paired devices, console sign-in, API tokens and webhook secrets. Without them, a restore keeps this PC’s own."
    />
    {#if secrets}
      <Banner tone="warn">Anyone with this file can pose as this PC to your paired devices and sign in to this console. Keep it private.</Banner>
    {/if}
    <div class="row">
      <Button icon="download" busy={exporting} disabled={restoring} onclick={download}>Download backup</Button>
      <Button icon="upload" busy={checking} disabled={restoring || exporting} onclick={() => fileInput?.click()}>Restore from file</Button>
    </div>
    <input
      bind:this={fileInput}
      class="visually-hidden"
      type="file"
      accept=".zip,application/zip"
      tabindex="-1"
      aria-label="Restore from file"
      onchange={check}
    />
    {#if problem}
      <Banner tone="danger" title="This file can’t be restored" live>{problem}</Banner>
    {/if}
    {#if late}
      <Banner tone="warn" live>Rubylight has not come back after 90 seconds. Check the PC, then reload this page.</Banner>
    {/if}
    {#if inspection}
      <div class="review" aria-live="polite">
        <p class="note">{restoreNote(inspection)}</p>
        <dl class="facts">
          {#each inspection.summary.changes as change (change.item)}
            <dt>{changeLabel(change.item)}</dt>
            <dd>{change.detail}</dd>
          {/each}
        </dl>
        <div class="row">
          <Button variant={inspection.summary.secrets ? 'danger' : 'primary'} busy={restoring} onclick={restore}>Restore and restart</Button>
          <Button variant="ghost" disabled={restoring} onclick={() => (inspection = null)}>Cancel</Button>
        </div>
      </div>
    {/if}
  </div>
</Panel>

<style>
  .note {
    font-size: var(--text-sm);
  }
  .review {
    display: grid;
    gap: var(--space-3);
  }
</style>
