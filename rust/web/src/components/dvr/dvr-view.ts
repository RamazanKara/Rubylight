/**
 * What the Clips page says about Stream DVR, as plain functions: the reason sentences (design 05
 * section 5.2), the live state's word and status, what each button may do now, and how a clip and
 * the quota read. The Svelte parts only lay these out, so DvrClips.test.mjs checks them directly.
 */
import type { Status } from '../StatusPill.svelte';
import { ApiError, type ClipSummary, type ConsoleDvrStatus, type DvrReason, type DvrStatus } from '../../lib/api';

/** Where the Instant replay setting lives: the Settings page opens it and scrolls to the row. */
export const DVR_SETTING_HREF = '/settings/picture?setting=dvr_enabled';
/** The Space for clips setting, linked from the near-full quota notice. */
export const DVR_QUOTA_HREF = '/settings/picture?setting=dvr_quota_gb';

/** One sentence per reason: what is wrong and what to do. */
const REASONS: Record<DvrReason, string> = {
  none: '',
  disabled: 'Instant Replay is turned off on this PC. Enable Stream DVR in Rubylight settings to save clips.',
  pyrowave: 'Clips cannot be saved while streaming with PyroWave. Switch to H.264, HEVC, or AV1 to use Stream DVR.',
  disk_full: 'Not enough free disk space on the host PC to save this clip. Free up space or delete older clips.',
  quota_exceeded: 'Not enough free disk space on the host PC to save this clip. Free up space or delete older clips.',
  no_keyframe_yet: 'Waiting for the stream to capture a full frame. Try saving again in a moment.',
  audio_codec_unsupported: 'Clips require Opus audio. Switch audio codec from passthrough to Opus to record clips.',
  busy: 'Another clip is being saved. Try again once it is done.',
  permission_denied: 'This sign-in may not save clips on this PC.',
  io_error: 'The clip could not be written to the clips folder. Check that the folder exists and its drive is connected.',
  not_streaming: 'Nothing is streaming. Clips can be saved while a device streams.',
  unknown: 'Clips are not available right now.',
};

/** The sentence for a reason; a reason a newer host adds reads as `unknown`. */
export function reasonText(reason: string | null | undefined): string {
  if (!reason) return '';
  return REASONS[reason as DvrReason] ?? REASONS.unknown;
}

/** Why an action was refused: the reason's sentence, else the host's own words. */
export function refusalText(failure: unknown): string {
  if (failure instanceof ApiError && failure.reason && failure.reason !== 'none' && failure.reason in REASONS) {
    return reasonText(failure.reason);
  }
  return failure instanceof Error ? failure.message : String(failure);
}

/** The live panel's status pill: a word, never colour alone. */
export function liveStatus(dvr: ConsoleDvrStatus): { status: Status; label: string } {
  if (!dvr.enabled || dvr.reason === 'disabled') return { status: 'offline', label: 'Off' };
  switch (dvr.state) {
    case 'armed':
      return { status: 'ready', label: 'Replay ready' };
    case 'recording':
      return { status: 'streaming', label: 'Recording' };
    case 'saving':
      return { status: 'busy', label: 'Saving a clip' };
    case 'error':
      return { status: 'error', label: 'Cannot save' };
    case 'off':
      return { status: 'idle', label: dvr.reason === 'not_streaming' ? 'Not streaming' : 'Off' };
    default:
      return { status: 'warning', label: 'Unavailable' };
  }
}

/** The sentence under the pill: how much replay is ready, the recording's time, or why not. */
export function liveLine(dvr: ConsoleDvrStatus): string {
  if (!dvr.enabled) return reasonText('disabled');
  if (dvr.reason !== 'none' && dvr.state !== 'armed' && dvr.state !== 'recording') return reasonText(dvr.reason);
  const ready = `Last ${Math.min(dvr.ring_buffered_secs, dvr.ring_capacity_secs || dvr.ring_buffered_secs)} s of ${dvr.ring_capacity_secs} s ready to save.`;
  if (dvr.state === 'recording' || dvr.recording) return `Recording ${clock(dvr.recording_elapsed_secs)}. ${ready}`;
  if (dvr.state === 'saving') return 'Writing the clip to the clips folder…';
  return ready;
}

/** Whether Save clip can be pressed now. */
export function canSave(dvr: DvrStatus & { enabled?: boolean }): boolean {
  return dvr.enabled !== false && (dvr.state === 'armed' || dvr.state === 'recording');
}

/** Whether Start recording can be pressed now; Stop stays available while a recording runs. */
export function canRecord(dvr: DvrStatus & { enabled?: boolean }): boolean {
  if (dvr.recording || dvr.state === 'recording') return true;
  return dvr.enabled !== false && dvr.state === 'armed';
}

/** How full the replay buffer is, 0 to 1. */
export function bufferShare(dvr: DvrStatus): number {
  if (!dvr.ring_capacity_secs) return 0;
  return Math.min(1, Math.max(0, dvr.ring_buffered_secs / dvr.ring_capacity_secs));
}

/** Seconds as a timer: 02:14, or 1:02:14 past an hour. */
export function clock(seconds: number): string {
  const total = Math.max(0, Math.floor(seconds));
  const pad = (value: number) => String(value).padStart(2, '0');
  const hours = Math.floor(total / 3600);
  const rest = `${pad(Math.floor((total % 3600) / 60))}:${pad(total % 60)}`;
  return hours ? `${hours}:${rest}` : rest;
}

/** A clip's length: 45 s, 1 min 30 s, 1 h 2 min. */
export function clipLength(ms: number): string {
  const seconds = Math.round(ms / 1000);
  if (seconds < 60) return `${seconds} s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return seconds % 60 ? `${minutes} min ${seconds % 60} s` : `${minutes} min`;
  return `${Math.floor(minutes / 60)} h ${minutes % 60} min`;
}

/** Sizes as Windows shows them, in 1024 steps: 84 MB, 4.2 GB. */
export function size(bytes: number): string {
  if (bytes < 1024 ** 2) return `${Math.max(0, Math.round(bytes / 1024))} KB`;
  if (bytes < 1024 ** 3) return `${Math.round(bytes / 1024 ** 2)} MB`;
  const gb = bytes / 1024 ** 3;
  return `${gb < 10 ? gb.toFixed(1) : Math.round(gb)} GB`;
}

/** "4.2 GB of 20 GB used". */
export function quotaText(used: number, limit: number): string {
  return limit > 0 ? `${size(used)} of ${size(limit)} used` : `${size(used)} used`;
}

/** The quota used, 0 to 1. */
export function quotaShare(used: number, limit: number): number {
  return limit > 0 ? Math.min(1, Math.max(0, used / limit)) : 0;
}

/** From 90% the bar warns: the oldest replays go next. */
export const QUOTA_NEAR_FULL = 0.9;
export function quotaNearFull(used: number, limit: number): boolean {
  return limit > 0 && used / limit >= QUOTA_NEAR_FULL;
}

/** The codec as the console names it elsewhere. */
export function codecLabel(codec: string): string {
  switch (codec.toLowerCase()) {
    case 'h264':
      return 'H.264';
    case 'hevc':
    case 'h265':
      return 'HEVC';
    case 'av1':
      return 'AV1';
    default:
      return codec ? codec.toUpperCase() : 'Unknown';
  }
}

/** "2560×1440 · 120 fps"; what the host did not record is left out. */
export function pictureText(clip: Pick<ClipSummary, 'width' | 'height' | 'fps'>): string {
  const parts = [];
  if (clip.width && clip.height) parts.push(`${clip.width}×${clip.height}`);
  if (clip.fps > 0) parts.push(`${Number.isInteger(clip.fps) ? clip.fps : clip.fps.toFixed(1)} fps`);
  return parts.join(' · ') || '–';
}

/** A replay or a recording; a kind a newer host adds reads as a clip. */
export function kindLabel(kind: string): string {
  return kind === 'manual' ? 'Recording' : kind === 'replay' ? 'Replay' : 'Clip';
}

/** When the clip was saved, as a local date and time. */
export function savedAt(unixMs: number): string {
  return unixMs ? new Date(unixMs).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' }) : '–';
}

/** The clip's name in a dialog or a toast. */
export function clipName(clip: Pick<ClipSummary, 'app_title' | 'kind'>): string {
  return clip.app_title || (clip.kind === 'manual' ? 'Recording' : 'Clip');
}

/** "Clip saved (60 s, 84 MB)." */
export function savedNotice(clip: Pick<ClipSummary, 'duration_ms' | 'size_bytes' | 'kind'>): string {
  return `${clip.kind === 'manual' ? 'Recording saved' : 'Clip saved'} (${clipLength(clip.duration_ms)}, ${size(clip.size_bytes)}).`;
}
