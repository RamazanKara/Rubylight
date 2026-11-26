<script lang="ts">
  /**
   * Stream DVR while a device streams: the state in a word, how much replay is ready or how long
   * the recording runs, and Save clip and Start or Stop recording. When clips are unavailable the
   * buttons stay in place, disabled, under the reason in plain words (with the setting's link when
   * it is off). The page polls `GET /api/dvr` and passes it in; this part only lays it out.
   */
  import Button from '../Button.svelte';
  import LoadState from '../LoadState.svelte';
  import Panel from '../Panel.svelte';
  import StatusPill from '../StatusPill.svelte';
  import DvrMeter from './DvrMeter.svelte';
  import type { ConsoleDvrStatus } from '../../lib/api';
  import {
    DVR_SETTING_HREF,
    bufferShare,
    canRecord,
    canSave,
    clock,
    liveLine,
    liveStatus,
    reasonText,
  } from './dvr-view';

  let {
    dvr,
    error = '',
    saving = false,
    recordingBusy = false,
    onsave,
    onrecord,
    onretry,
  }: {
    dvr: ConsoleDvrStatus | null;
    error?: string;
    /** A Save clip request is on its way. */
    saving?: boolean;
    /** A Start or Stop recording request is on its way. */
    recordingBusy?: boolean;
    onsave: () => void;
    onrecord: (action: 'start' | 'stop') => void;
    onretry: () => Promise<void>;
  } = $props();

  const pill = $derived(dvr ? liveStatus(dvr) : null);
  const recording = $derived(!!dvr && (dvr.recording || dvr.state === 'recording'));
  const saveable = $derived(!!dvr && canSave(dvr));
  const recordable = $derived(!!dvr && canRecord(dvr));
  /** Why the buttons are disabled, for their tooltip; nothing while they work. */
  const why = $derived(dvr && !saveable ? (dvr.enabled ? reasonText(dvr.reason) : reasonText('disabled')) : '');
  const tone = $derived(dvr?.state === 'error' ? 'danger' : dvr?.state === 'unsupported' ? 'warn' : '');
  const buffering = $derived(!!dvr && dvr.enabled && (dvr.state === 'armed' || dvr.state === 'recording' || dvr.state === 'saving'));
</script>

<Panel title="Instant replay" description="Save the last moments of the stream, or record it, on this PC without encoding again.">
  {#if dvr === null}
    <LoadState {error} what="Stream DVR's state" {onretry} />
  {:else}
    {#if error}<LoadState {error} what="Stream DVR's state" stale {onretry} />{/if}
    <div class="dvr-live" data-state={dvr.state}>
      <div class="state">
        {#if pill}<StatusPill status={pill.status} label={pill.label} />{/if}
        {#if recording}<span class="timer num" aria-label="Recording time">{clock(dvr.recording_elapsed_secs)}</span>{/if}
      </div>

      {#if !dvr.enabled}
        <p class="notice">
          <span>{reasonText('disabled')}</span>
          <Button size="sm" href={DVR_SETTING_HREF}>Open the Instant replay setting</Button>
        </p>
      {:else if tone}
        <p class="notice {tone}" role="status">{liveLine(dvr)}</p>
      {:else}
        <p class="line" role="status">{liveLine(dvr)}</p>
      {/if}

      {#if buffering}
        <DvrMeter
          label="Replay buffer"
          text={`${Math.min(dvr.ring_buffered_secs, dvr.ring_capacity_secs)} s of ${dvr.ring_capacity_secs} s`}
          share={bufferShare(dvr)}
          tone={recording ? 'live' : 'accent'}
        />
      {/if}

      <div class="actions">
        <Button
          variant="primary"
          icon="film"
          busy={saving || dvr.state === 'saving'}
          disabled={!saveable || saving}
          title={why || undefined}
          onclick={onsave}
        >
          Save clip
        </Button>
        {#if recording}
          <Button icon="stop" busy={recordingBusy} disabled={recordingBusy} onclick={() => onrecord('stop')}>Stop recording</Button>
        {:else}
          <Button
            icon="record"
            busy={recordingBusy}
            disabled={!recordable || recordingBusy}
            title={why || undefined}
            onclick={() => onrecord('start')}
          >
            Start recording
          </Button>
        {/if}
      </div>
    </div>
  {/if}
</Panel>

<style>
  .dvr-live {
    display: grid;
    gap: var(--space-4);
    min-width: 0;
  }
  .state {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-3);
  }
  .timer {
    font-size: var(--text-lg);
    font-weight: var(--weight-semibold);
    color: var(--live);
  }
  .line,
  .notice {
    margin: 0;
    max-width: 70ch;
  }
  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-3);
  }
</style>
