<script lang="ts">
  /**
   * A saved clip played in the browser, in a dialog: the host serves it as `video/mp4` with range
   * requests, so the player can seek. The video loads only while the dialog is open.
   */
  import Button from '../Button.svelte';
  import Dialog from '../Dialog.svelte';
  import { clipDownloadUrl, clipVideoUrl, type ClipSummary } from '../../lib/api';
  import { clipLength, clipName, codecLabel, pictureText, savedAt } from './dvr-view';

  let { clip = $bindable(null) }: { clip: ClipSummary | null } = $props();
  let open = $state(false);
  let failed = $state(false);
  $effect(() => {
    open = clip !== null;
    failed = false;
  });
</script>

<Dialog
  bind:open
  size="lg"
  title={clip ? clipName(clip) : 'Clip'}
  description={clip ? `${savedAt(clip.created_unix_ms)} · ${clipLength(clip.duration_ms)} · ${codecLabel(clip.codec)}${clip.hdr ? ' HDR' : ''} · ${pictureText(clip)}` : undefined}
  onclose={() => (clip = null)}
>
  {#if clip && open}
    <!-- svelte-ignore a11y_media_has_caption -->
    <video class="player" src={clipVideoUrl(clip.id)} controls autoplay preload="metadata" onerror={() => (failed = true)}></video>
    {#if failed}
      <p class="notice warn">This browser cannot play the clip here. Download it to watch it in a video player.</p>
    {/if}
  {/if}
  {#snippet actions()}
    {#if clip}<Button icon="download" href={clipDownloadUrl(clip.id)}>Download</Button>{/if}
    <Button variant="primary" onclick={() => (clip = null)}>Close</Button>
  {/snippet}
</Dialog>

<style>
  .player {
    display: block;
    inline-size: 100%;
    max-block-size: 70vh;
    border-radius: var(--radius);
    background: var(--media-black);
  }
  .notice {
    margin: var(--space-3) 0 0;
  }
</style>
