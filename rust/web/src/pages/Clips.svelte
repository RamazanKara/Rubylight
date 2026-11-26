<script lang="ts">
  /**
   * Clips: Stream DVR in the console. The live panel (instant replay and recording while a device
   * streams, polled every 2 s) above the saved clips with the quota, previews, downloads, deletes
   * and the clips folder. Parts live in components/dvr/; their words in components/dvr/dvr-view.ts.
   */
  import { onMount } from 'svelte';
  import Button from '../components/Button.svelte';
  import PageHeader from '../components/PageHeader.svelte';
  import ClipPreview from '../components/dvr/ClipPreview.svelte';
  import ClipsPanel from '../components/dvr/ClipsPanel.svelte';
  import DvrLivePanel from '../components/dvr/DvrLivePanel.svelte';
  import { clipName, refusalText, savedAt, savedNotice } from '../components/dvr/dvr-view';
  import { api, type ClipSummary, type ConsoleDvrStatus } from '../lib/api';
  import { confirm, failed, notify } from '../lib/feedback.svelte';
  import { poll } from '../lib/format';

  let dvr = $state<ConsoleDvrStatus | null>(null);
  let dvrError = $state('');
  let clips = $state<ClipSummary[] | null>(null);
  let clipsError = $state('');
  let cursor = $state<string | null>(null);
  let quota = $state({ used: 0, limit: 0 });
  let saving = $state(false);
  let recordingBusy = $state(false);
  let loadingMore = $state(false);
  let deleting = $state<string | null>(null);
  let opening = $state(false);
  let previewing = $state<ClipSummary | null>(null);

  const reason = (value: unknown) => (value instanceof Error ? value.message : String(value));

  // Only the newest poll may land, as the Sessions page's stream poll does.
  let latest = 0;
  async function loadStatus() {
    const id = ++latest;
    try {
      const reply = await api.dvr.status();
      if (id !== latest) return;
      dvr = reply;
      dvrError = '';
      if (reply.quota_limit_bytes) quota = { used: reply.quota_used_bytes, limit: reply.quota_limit_bytes };
    } catch (failure) {
      if (id !== latest) return;
      dvrError = reason(failure);
    }
  }

  /** The first page again: after a save, a stop or a delete, and on the slow poll. */
  async function loadClips() {
    try {
      const reply = await api.dvr.clips();
      clips = reply.clips;
      cursor = reply.next_cursor;
      quota = { used: reply.quota_used_bytes, limit: reply.quota_limit_bytes };
      clipsError = '';
    } catch (failure) {
      clipsError = reason(failure);
    }
  }

  async function loadMore() {
    if (!cursor || loadingMore) return;
    loadingMore = true;
    try {
      const reply = await api.dvr.clips(cursor);
      const known = new Set((clips ?? []).map((clip) => clip.id));
      clips = [...(clips ?? []), ...reply.clips.filter((clip) => !known.has(clip.id))];
      cursor = reply.next_cursor;
      quota = { used: reply.quota_used_bytes, limit: reply.quota_limit_bytes };
    } catch (failure) {
      failed('Loading older clips failed', failure);
    } finally {
      loadingMore = false;
    }
  }

  async function save() {
    if (saving) return;
    saving = true;
    try {
      const reply = await api.dvr.saveClip();
      notify(savedNotice(reply.clip), 'ok');
      await Promise.all([loadStatus(), loadClips()]);
    } catch (failure) {
      notify(`The clip was not saved: ${refusalText(failure)}`, 'danger');
      void loadStatus();
    } finally {
      saving = false;
    }
  }

  async function record(action: 'start' | 'stop') {
    if (recordingBusy) return;
    recordingBusy = true;
    try {
      const reply = await api.dvr.record(action);
      if (dvr) dvr = { ...dvr, ...reply.dvr };
      if (reply.clip) {
        notify(savedNotice(reply.clip), 'ok');
        await loadClips();
      } else if (action === 'start') {
        notify('Recording started.', 'ok');
      }
      void loadStatus();
    } catch (failure) {
      notify(`${action === 'start' ? 'Recording did not start' : 'Recording did not stop'}: ${refusalText(failure)}`, 'danger');
      void loadStatus();
    } finally {
      recordingBusy = false;
    }
  }

  async function remove(clip: ClipSummary) {
    const ok = await confirm({
      title: `Delete ${clipName(clip)}?`,
      message: `The clip saved ${savedAt(clip.created_unix_ms)} is deleted from this PC. This cannot be undone.`,
      confirm: 'Delete clip',
      danger: true,
    });
    if (!ok) return;
    deleting = clip.id;
    try {
      await api.dvr.remove(clip.id);
      clips = (clips ?? []).filter((candidate) => candidate.id !== clip.id);
      if (previewing?.id === clip.id) previewing = null;
      notify(`Deleted ${clipName(clip)}.`, 'ok');
      await loadClips();
    } catch (failure) {
      failed(`Deleting ${clipName(clip)} failed`, failure);
    } finally {
      deleting = null;
    }
  }

  async function openFolder() {
    opening = true;
    try {
      await api.dvr.openFolder();
      notify('The clips folder opened on this PC’s desktop.', 'ok');
    } catch (failure) {
      failed('Opening the clips folder failed', failure);
    } finally {
      opening = false;
    }
  }

  onMount(() => {
    const stopStatus = poll(loadStatus, 2000);
    const stopClips = poll(loadClips, 15_000);
    return () => {
      stopStatus();
      stopClips();
    };
  });
</script>

<div class="page">
  <PageHeader title="Clips" subtitle="Instant replays and recordings saved on this PC, newest first.">
    {#snippet actions()}
      <Button icon="folder" busy={opening} onclick={openFolder}>Open clips folder</Button>
    {/snippet}
  </PageHeader>

  <DvrLivePanel
    {dvr}
    error={dvrError}
    {saving}
    {recordingBusy}
    onsave={save}
    onrecord={record}
    onretry={loadStatus}
  />

  <ClipsPanel
    {clips}
    error={clipsError}
    {quota}
    {dvr}
    more={!!cursor}
    {deleting}
    {loadingMore}
    onpreview={(clip) => (previewing = clip)}
    ondelete={remove}
    onmore={loadMore}
    onretry={loadClips}
  />
</div>

<ClipPreview bind:clip={previewing} />

<style>
  .page {
    display: grid;
    gap: var(--space-5);
    min-width: 0;
  }
</style>
