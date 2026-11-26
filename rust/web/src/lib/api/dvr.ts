/**
 * Stream DVR (instant replay and recording) for the console: the types of `/api/dvr*` and its
 * calls. The shapes follow the native API's `dvr` module (client/protocol/native-api/src/dvr.rs,
 * docs/native-api.md "Stream DVR"); the console's status adds `enabled` and `clips_dir`.
 * `api.ts` binds the calls to its request helper as `api.dvr`.
 */

/** What the replay buffer does now. A state a newer host adds reads as `unknown`. */
export type DvrState = 'off' | 'armed' | 'recording' | 'saving' | 'unsupported' | 'error' | 'unknown';

/** Why clips are unavailable or an action was refused; `none` while all is well. */
export type DvrReason =
  | 'none'
  | 'disabled'
  | 'pyrowave'
  | 'disk_full'
  | 'quota_exceeded'
  | 'no_keyframe_yet'
  | 'busy'
  | 'permission_denied'
  | 'audio_codec_unsupported'
  | 'io_error'
  | 'not_streaming'
  | 'unknown';

export interface DvrStatus {
  state: DvrState;
  reason: DvrReason;
  ring_buffered_secs: number;
  ring_capacity_secs: number;
  recording: boolean;
  recording_elapsed_secs: number;
  quota_used_bytes: number;
  quota_limit_bytes: number;
  clips_dir_available: boolean;
  container: string;
}

/** `GET /api/dvr`: the stream's DVR state, whether the setting is on and the resolved clips folder. */
export interface ConsoleDvrStatus extends DvrStatus {
  enabled: boolean;
  clips_dir: string;
}

/** `replay` is a saved instant replay, `manual` a recording. */
export type ClipKind = 'replay' | 'manual' | 'unknown';

export interface ClipSummary {
  /** `[A-Za-z0-9_-]{8,64}`. */
  id: string;
  app_title: string;
  created_unix_ms: number;
  duration_ms: number;
  width: number;
  height: number;
  fps: number;
  /** `h264`, `hevc` or `av1`. */
  codec: string;
  hdr: boolean;
  container: string;
  size_bytes: number;
  kind: ClipKind;
  /** Kept from a recording the host could not finish. */
  recovered: boolean;
}

export interface ClipList {
  /** Newest first. */
  clips: ClipSummary[];
  next_cursor: string | null;
  quota_used_bytes: number;
  quota_limit_bytes: number;
}

export interface ClipSaved {
  status: true;
  clip: ClipSummary;
}

export interface RecordReply {
  status: true;
  dvr: DvrStatus;
  /** The finished recording after a stop. */
  clip: ClipSummary | null;
}

/** The request helper `api.ts` passes in: parsed JSON, or an `ApiError` (with `reason` on a 409). */
export type DvrCall = <T>(method: 'GET' | 'POST' | 'DELETE', path: string, body?: unknown, timeout?: number) => Promise<T>;

/** Saving waits until the host has written the file, which takes longer than a read. */
export const DVR_SAVE_TIMEOUT_MS = 60_000;
/** How many clips one page of the list asks for. */
export const CLIPS_PAGE = 50;

const clipPath = (id: string) => `/api/dvr/clips/${encodeURIComponent(id)}`;

/** The clip as `video/mp4` for an inline `<video>` (the session cookie goes with it). */
export const clipVideoUrl = (id: string) => `${clipPath(id)}/video`;
/** The clip as a download. */
export const clipDownloadUrl = (id: string) => `${clipPath(id)}/download`;

/** The calls, bound to `call`. */
export function dvrEndpoints(call: DvrCall) {
  return {
    status: () => call<ConsoleDvrStatus>('GET', '/api/dvr'),
    clips: (cursor?: string | null, limit = CLIPS_PAGE) => {
      const query = new URLSearchParams({ limit: String(limit) });
      if (cursor) query.set('cursor', cursor);
      return call<ClipList>('GET', `/api/dvr/clips?${query}`);
    },
    /** Save the replay buffer; without `durationSecs` the host saves all of it. */
    saveClip: (durationSecs?: number) =>
      call<ClipSaved>('POST', '/api/dvr/clip', durationSecs ? { duration_secs: durationSecs } : {}, DVR_SAVE_TIMEOUT_MS),
    record: (action: 'start' | 'stop') => call<RecordReply>('POST', '/api/dvr/record', { action }, DVR_SAVE_TIMEOUT_MS),
    remove: (id: string) => call<{ status: true }>('DELETE', clipPath(id)),
    openFolder: () => call<{ status: true }>('POST', '/api/dvr/open-folder', {}),
  };
}
