// Typed client for the Rubylight admin API (/api on the console port).
// Sessions live in HttpOnly cookies; writes carry the session's CSRF token.
import { polled } from './format';

export class ApiError extends Error {
  constructor(
    readonly status: number,
    message: string,
  ) {
    super(message);
  }
}

/**
 * How long a request waits for the host's whole answer before it fails.
 * The host finishes the work anyway, so actions that wait on the PC get more.
 */
export const REQUEST_TIMEOUT_MS = 20_000;
/** Ending streams, tearing down displays, Playnite, a network share or a cover download. */
const ACTION_TIMEOUT_MS = 60_000;
/** An app's preparation and undo commands (up to two minutes each), or a library sync. */
const SLOW_TIMEOUT_MS = 10 * 60_000;

/** The host did not answer in time, so the request was abandoned (status 0). */
export class TimeoutError extends ApiError {
  constructor(milliseconds: number) {
    super(0, `The host did not answer within ${Math.round(milliseconds / 1000)} seconds. Check that Rubylight is running, then try again.`);
  }
}

/** A response and its body, both within `timeout` ms. */
async function exchange(path: string, init: RequestInit, timeout: number): Promise<{ response: Response; text: string }> {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timeout);
  try {
    const response = await fetch(path, { ...init, credentials: 'same-origin', signal: controller.signal });
    return { response, text: await response.text() };
  } catch (error) {
    if (controller.signal.aborted) throw new TimeoutError(timeout);
    throw error;
  } finally {
    clearTimeout(timer);
  }
}

let csrfToken = '';
let onSignedOut: (() => void) | null = null;

/** Called when the session can no longer be refreshed. */
export function setSignedOutHandler(handler: () => void) {
  onSignedOut = handler;
}
export function setCsrfToken(token: string) {
  csrfToken = token;
}

async function refreshCsrf(): Promise<void> {
  const { response, text } = await exchange('/api/csrf-token', {}, REQUEST_TIMEOUT_MS);
  if (response.ok) {
    const body = JSON.parse(text) as { csrf_token: string };
    csrfToken = body.csrf_token;
  }
}

/**
 * The session's CSRF token, for a request that cannot carry the header: the stream page's
 * WebSocket sends it as a subprotocol. A browser whose sign-in lapsed gets an anonymous token
 * here, which the host then refuses; it is not kept for the other requests.
 */
export async function sessionCsrfToken(): Promise<string> {
  const { response, text } = await exchange('/api/csrf-token', {}, REQUEST_TIMEOUT_MS);
  if (!response.ok) throw new ApiError(response.status, `The host did not give out the session's CSRF token (HTTP ${response.status}).`);
  const token = (JSON.parse(text) as { csrf_token?: string }).csrf_token;
  if (!token) throw new ApiError(response.status, 'The host gave out no CSRF token.');
  return token;
}

// One refresh at a time: the host rotates the refresh token, so a second
// request with the old one would fail and sign the user out.
let refreshing: Promise<boolean> | null = null;
function refreshSession(): Promise<boolean> {
  refreshing ??= (async () => {
    const { response, text } = await exchange('/api/auth/refresh', { method: 'POST' }, REQUEST_TIMEOUT_MS);
    if (!response.ok) return false;
    const body = JSON.parse(text) as IssuedSession;
    csrfToken = body.csrf_token;
    return true;
  })().finally(() => {
    refreshing = null;
  });
  return refreshing;
}

type Method = 'GET' | 'POST' | 'PUT' | 'PATCH' | 'DELETE';

async function send(
  method: Method,
  path: string,
  body: unknown,
  timeout: number,
  retried = false,
  background = polled(),
): Promise<{ response: Response; text: string }> {
  const headers: Record<string, string> = {};
  if (background) headers['X-Rubylight-Poll'] = '1';
  if (method !== 'GET') {
    if (!csrfToken) await refreshCsrf();
    headers['X-CSRF-Token'] = csrfToken;
  }
  if (body !== undefined) headers['Content-Type'] = 'application/json';
  const reply = await exchange(
    path,
    { method, headers, body: body === undefined ? undefined : JSON.stringify(body) },
    timeout,
  );
  if (retried || path.startsWith('/api/auth/')) return reply;
  if (reply.response.status === 401) {
    if (await refreshSession()) return send(method, path, body, timeout, true, background);
    onSignedOut?.();
    return reply;
  }
  // CSRF tokens are regenerated when the host restarts.
  if (reply.response.status === 400 && method !== 'GET' && reply.text.includes('CSRF token required')) {
    await refreshCsrf();
    return send(method, path, body, timeout, true, background);
  }
  return reply;
}

async function json<T>(method: Method, path: string, body?: unknown, timeout = REQUEST_TIMEOUT_MS): Promise<T> {
  const { response, text } = await send(method, path, body, timeout);
  let parsed: unknown = undefined;
  try {
    parsed = text ? JSON.parse(text) : undefined;
  } catch {
    // Some errors are plain text.
  }
  if (!response.ok) {
    const message =
      (parsed as { error?: string } | undefined)?.error ?? (text || `${response.status} ${response.statusText}`);
    throw new ApiError(response.status, message);
  }
  return parsed as T;
}

const get = <T>(path: string, timeout = REQUEST_TIMEOUT_MS) => json<T>('GET', path, undefined, timeout);
const post = <T>(path: string, body: unknown = {}, timeout = REQUEST_TIMEOUT_MS) => json<T>('POST', path, body, timeout);

// ---------- types ----------

export interface Ok {
  status: true;
}
export interface AuthStatus {
  status: true;
  authenticated: boolean;
  credentials_configured: boolean;
  login_required: boolean;
  /** On this PC while signed out: the program that sets a new sign-in with --creds. */
  creds_program?: string;
}
export interface IssuedSession {
  status: true;
  access_token: string;
  refresh_token: string;
  csrf_token: string;
  expires_in: number;
  refresh_expires_in: number;
  remember_me: boolean;
}
export interface WebSession {
  id: string;
  current: boolean;
  username: string;
  created_at: number;
  expires_at: number;
  refresh_expires_at: number;
  last_seen: number;
  remember_me: boolean;
  user_agent: string;
  remote_address: string;
  device_label: string;
}
export interface TokenScope {
  path: string;
  methods: string[];
}
export interface ApiToken {
  hash: string;
  username: string;
  created_at: number | string;
  scopes: TokenScope[];
}
export type ConfigValue = string | number | boolean | null | unknown[] | Record<string, unknown>;
export type Config = Record<string, ConfigValue>;

export interface CaptureDisplay {
  device_id: string;
  display_name: string;
  friendly_name: string;
  width: number;
  height: number;
  x: number;
  y: number;
  primary: boolean;
  adapter: string;
}
export interface AudioEndpoint {
  id: string;
  name: string;
  description: string;
  adapter: string;
  default: boolean;
  virtual_sink: boolean;
}
export interface Gpu {
  name: string;
  vendor: number;
  dedicated_memory: number;
  luid: [number, number];
  pnp_id: string | null;
}
export interface Metadata {
  status: true;
  platform: 'windows';
  version: string;
  host_name: string;
  pc_address: string;
  pc_addresses: string[];
  paired_devices: number;
  enable_pairing: boolean;
  warnings: { code: string; message: string }[];
  encoder_status: { state: 'checking' | 'failed' | 'ready'; h264: boolean; hevc: boolean; av1: boolean; pyrowave: boolean };
  capture_status: {
    configured_backend: string;
    virtual_display_configured: boolean;
    displays: CaptureDisplay[] | null;
    error: string | null;
  };
  virtual_display: { capable: boolean; ready: boolean; reason: string; protocol: string };
  gpus: Gpu[] | null;
  gpu_error: string | null;
  audio_sinks: AudioEndpoint[] | null;
  audio_error: string | null;
  audio_enabled: boolean;
  features: { pyrowave: boolean; virtual_display: boolean; [key: string]: boolean };
  credentials_exists: boolean;
  webrtc?: { enabled: boolean; running: boolean; discovery: boolean; port: number; media_port: number; error: string | null };
}

export interface PrepCommand {
  do: string;
  undo: string;
  elevated: boolean;
}
export interface App {
  name: string;
  cmd: string;
  'working-dir': string;
  'prep-cmd': PrepCommand[];
  uuid?: string;
  'image-path'?: string;
  detached?: string[];
  'config-overrides'?: Record<string, string | number | boolean>;
  /** competitive, smoothness, quality or custom. */
  'radeon-profile'?: string;
  /** The Radeon settings of a custom profile: setting id to value. */
  'radeon-settings'?: Record<string, string | number | boolean>;
  [extra: string]: unknown;
}
export interface AppsDocument {
  status: true;
  apps: App[];
  env?: Record<string, string>;
}

export const PERM = {
  LIST_APPS: 0x01000000,
  VIEW_STREAMS: 0x02000000,
  LAUNCH_APPS: 0x04000000,
  CONTROLLER: 0x100,
  TOUCH: 0x200,
  PEN: 0x400,
  MOUSE: 0x800,
  KEYBOARD: 0x1000,
  CLIPBOARD_WRITE: 0x10000,
  CLIPBOARD_READ: 0x20000,
  FILE_UPLOAD: 0x40000,
  FILE_DOWNLOAD: 0x80000,
  SERVER_CMD: 0x100000,
} as const;
export const PERM_ALL = 0x071f1f00;
/** Full control: everything but file transfer, which a device is granted on its own. */
export const FULL_CONTROL = PERM_ALL & ~(PERM.FILE_UPLOAD | PERM.FILE_DOWNLOAD);

// Per-device settings: null clears one, so the host setting applies.
export interface Client {
  uuid: string;
  name: string;
  perm: number;
  enabled: boolean;
  connected: boolean;
  /** Set for a device paired over WebRTC; the secret never leaves the host. */
  webrtc?: { clientId: string };
  network_loss_routes?: { address: string; expires_in_seconds: number; sessions_until_probe: number }[];
  /** The User-Agent of the device's latest /serverinfo since the host started. Read-only. */
  client_agent?: string | null;
  /** A known problem with this client build. The host never refuses the device for it. */
  client_warning?: { problem: string; link: string } | null;
  output_name_override?: string | null;
  display_mode?: string | null;
  always_use_virtual_display?: boolean | string | null;
  remember_display_mode?: boolean | string | null;
  remembered_resolution?: string | null;
  remembered_scale?: number | string | null;
  background_stream?: boolean | string | null;
  virtual_display_mode?: string | null;
  virtual_display_layout?: string | null;
  virtual_display_layout_set_by_user?: boolean | null;
  prefer_10bit_sdr?: boolean | string | null;
  hdr_profile?: string | null;
  allow_client_commands?: boolean | string | null;
  /** Host control (Radeon settings, close game). Sent as true or null; the host refuses anything else. */
  host_control?: boolean | string | null;
  /** Sleep PC from this device. Sent as true or null; the host refuses anything else. */
  allow_sleep?: boolean | string | null;
  /** USB forwarding from this device (3.0, not yet served). Sent as a boolean or null. */
  allow_usb?: boolean | null;
  /** Folder grants by folder id (3.0, not yet served). Null clears them. */
  shared_folders?: Record<string, 'read' | 'write'> | null;
  do?: unknown;
  undo?: unknown;
  [extra: string]: unknown;
}
export interface PendingPairing {
  uniqueid: string;
  name: string;
  age_seconds: number;
  /** The request comes from a WebRTC TV. */
  webrtc?: boolean;
}

export type LibrarySource = 'steam' | 'epic' | 'battlenet' | 'ea' | 'ubisoft' | 'gog' | 'microsoft' | 'local';
export interface DiscoveredGame {
  source: LibrarySource;
  id: string;
  uuid: string;
  name: string;
  install_dir: string;
  already_added: boolean;
  unavailable_reason: string | null;
  has_cover: boolean;
}
export interface LibraryDiscovery {
  status: true;
  sources: { source: LibrarySource; name: string; state: 'available' | 'unavailable' | 'error' | 'disabled'; errors: string[] }[];
  games: DiscoveredGame[];
}
export interface LibraryImportResult {
  source: LibrarySource;
  id: string;
  result: 'added' | 'already_present' | 'failed';
  error: string | null;
}

export interface SteamStatus {
  status: true;
  enabled: boolean;
  /** Whether Steam is installed on the host. */
  available: boolean;
  game_count: number;
  importable_game_count: number;
  selected_game_count: number;
  auto_sync: boolean;
  sync_all_installed: boolean;
  recent_games: number;
}

export interface PlayniteStatus {
  status: true;
  enabled: boolean;
  /** Playnite is running. */
  active: boolean;
  /** Playnite was found on the host. */
  available: boolean;
  /** The plugin is in Playnite's extensions. */
  installed: boolean;
  installed_version: string | null;
  packaged_version: string | null;
  update_available: boolean;
  auto_sync: boolean;
  game_count: number;
  synced_seconds_ago: number | null;
}

export interface PerfSample {
  fps: number;
  bitrate_mbps: number;
  encode_mean_ms: number;
  encode_max_ms: number;
  host_processing_mean_ms: number;
  host_processing_max_ms: number;
  frame_age_mean_ms: number;
}
/** Milliseconds a stage took, over the new pictures in the stage window. */
export interface StageTiming {
  mean_ms: number;
  p95_ms: number;
  p99_ms: number;
}
/** Where new pictures' time went on the host, from Windows' present to the last packet sent. */
export interface StageTimings {
  /** New pictures sent in the window; static repeats are not counted. */
  frames: number;
  window_seconds: number;
  capture: StageTiming;
  hold: StageTiming;
  claim: StageTiming;
  convert: StageTiming;
  encode: StageTiming;
  queue: StageTiming;
  packetize: StageTiming;
  send_queue: StageTiming;
  send: StageTiming;
  /** Present to the last packet sent: the stages above added up. */
  present_to_sent: StageTiming;
  /** The host latency Moonlight shows, for the same pictures. */
  moonlight_host: StageTiming;
}
export interface Performance {
  fps: number;
  bitrate_mbps: number;
  encode_mean_ms: number;
  encode_p95_ms: number;
  encode_p99_ms: number;
  host_processing_mean_ms: number;
  host_processing_p95_ms: number;
  host_processing_p99_ms: number;
  host_processing_max_ms: number;
  frame_age_mean_ms: number;
  frame_age_p95_ms: number;
  present_to_send_mean_ms: number;
  send_interval_p99_ms: number;
  stages?: StageTimings;
  sample_frames: number;
  history: PerfSample[];
}
export type Role = 'stream' | 'remote_monitor' | 'input_only';
export interface StreamSession {
  encoder: string;
  warnings: { code: string; message: string }[];
  uuid: string;
  device_name: string;
  width: number;
  height: number;
  /** The rate the stream is encoded at. */
  fps: number;
  /** The rate in the device's stream request; differs when Use the launch frame rate applies. */
  requested_fps?: number;
  video_format: 0 | 1 | 2 | 3;
  hdr: boolean;
  vrr: boolean;
  encoder_bitrate_kbps: number;
  /** The host's network bitrate control: `limited` and `floor` mean the picture runs below `ceiling_kbps` because the network lost packets. */
  link_rate?: {
    state: 'off' | 'full' | 'limited' | 'floor';
    reason: string;
    ceiling_kbps: number;
    current_kbps: number;
    delivered_kbps: number;
    /** The rate that last failed, or 0 when none is known. */
    bad_kbps: number;
    /** What the encoder was seen to send after a cut asked for less, or 0. The host does not cut below it. */
    encoder_floor_kbps: number;
    steps_down: number;
    probes: number;
    failed_probes: number;
    /** Milliseconds since the last change, or 0 before the first. */
    last_change_ms_ago: number;
  };
  /** Error correction (%) for the next video frame; adaptive FEC moves it. Null for PyroWave. */
  fec_percent?: number | null;
  /** PyroWave streams: severe detail loss is likely below this bitrate. */
  pyrowave_minimum_kbps: number | null;
  /** A clean-picture target from synthetic scenes, not a quality guarantee. */
  pyrowave_recommended_kbps: number | null;
  audio_channels: number;
  state: 'RUNNING' | 'STOPPING';
  frames_sent: number;
  bytes_sent: number;
  idr_requests: number;
  performance: Performance;
  uptime_seconds: number;
  role: Role;
}
export interface ConsoleRecovery {
  session_id: number;
  console_session_id: number;
  can_reattach: boolean;
  reason: string;
}

export interface SessionStatus {
  status: true;
  activeSessions: number;
  appRunning: boolean;
  appName: string;
  paused: boolean;
  running: boolean;
  app: { name: string; id: number } | null;
}
export interface DisplayDevice {
  device_id: string;
  display_name: string;
  friendly_name: string;
  hdr_supported: boolean;
  hdr_enabled: boolean;
  primary: boolean;
}
export interface GoldenStatus {
  status: true;
  exists: boolean;
  comparison_available: boolean;
  current_mismatch_reason: string;
}
export interface LimiterStatus {
  status: true;
  enabled: boolean;
  configured_provider: string;
  active_provider: string;
  rtss_available: boolean;
  resolved_path: string;
  path_exists: boolean;
  process_running: boolean;
  message: string;
}
export interface VulkanLayerStatus {
  status: true;
  installed: boolean;
  enabled: boolean;
  available: boolean;
  active: boolean;
}
export type CrashStatus =
  | { available: false; dismissed: false }
  | {
      available: true;
      filename: string;
      process: string;
      size_bytes: number;
      captured_at: string;
      age_seconds: number;
      dismissed: boolean;
    };
export interface Release {
  tag_name: string;
  name: string;
  html_url: string;
  prerelease: boolean;
  published_at: string;
  body: string;
}
export interface UpdatesState {
  status: true;
  checking: boolean;
  check_failed: boolean;
  check_error?: string | null;
  checked_at: number;
  /** Unix seconds of the next automatic check; null when automatic checks are off. */
  next_check_at?: number | null;
  /** The check is due but held back by an active or pending stream. */
  deferred?: boolean;
  releases: Release[];
  update_available?: boolean;
  latest_version?: string | null;
  install_supported: boolean;
  auto_update: boolean;
  update_check_interval: number;
  phase?: 'idle' | 'waiting' | 'downloading' | 'ready' | 'installing' | 'failed';
  queued_version?: string | null;
  /** Install now: does not wait for an idle host and ends a running stream. */
  now?: boolean;
  downloaded_bytes?: number;
  download_size?: number;
  error?: string | null;
  last_install?: { version: string; phase: string; error?: string } | null;
}
export interface BrowseEntry {
  name: string;
  path: string;
  type: 'directory' | 'file';
}
export interface BrowseListing {
  /** The folder listed; empty for the list of drives. */
  path: string;
  /** Empty at a drive's root, which goes back to the drives. */
  parent: string;
  entries: BrowseEntry[];
}
/** Which files a listing includes; folders are always listed. */
export type BrowseType = 'executable' | 'file' | 'directory';
export interface LogFinding {
  id: string;
  category: 'encoder' | 'capture' | 'network' | 'driver';
  level: 'warn' | 'error';
  title: string;
  explanation: string;
  fix: string;
  /** The troubleshooting guide's section for it. */
  link: string;
  count: number;
  first_seen: string;
  last_seen: string;
  /** The latest matching line, without its time and level. */
  sample: string;
}
export interface LogAnalysis {
  status: true;
  /** Newest first. */
  findings: LogFinding[];
  /** The first and last time in the part of the log that was read. */
  since: string | null;
  until: string | null;
  read_bytes: number;
}
export interface LogChunk {
  status: true;
  /** Byte offset to ask for next. */
  offset: number;
  size: number;
  /** The log was rotated or truncated since `offset`; `text` starts afresh. */
  reset: boolean;
  text: string;
}
/** One wake setting of an adapter; ids and states from a newer host are shown by their state. */
export interface WakeCheck {
  id: string;
  state: 'ok' | 'warning' | 'error' | 'unknown';
}
/** A physical adapter a magic packet can wake the PC through. */
export interface WakeTarget {
  mac: string;
  kind: 'ethernet' | 'wifi' | 'unknown';
  connected: boolean;
  /** This console's request came in through this adapter. */
  route: boolean;
  broadcasts: string[];
  state: 'ready' | 'needs_setup' | 'experimental' | 'unknown';
  checks: WakeCheck[];
}
export type IdleSleepState = 'off' | 'armed' | 'blocked' | 'unsupported' | 'unknown';
export interface PowerStatus {
  status: true;
  wake: {
    revision: number;
    /** The MAC `/serverinfo` reports; null when no physical adapter is connected. */
    primary: string | null;
    targets: WakeTarget[];
    system: {
      sleep: 's3' | 'none' | 'unknown';
      modern_standby: boolean;
      hibernate: boolean;
      fast_startup: 'active' | 'inactive' | 'off' | 'unknown';
      shutdown_wake: 'firmware_unknown' | 'unknown';
    };
  };
  idle_sleep: {
    state: IdleSleepState;
    /** 0 when off. */
    minutes: number;
    /** Why it is blocked or unsupported, or, while armed, what it waits for. */
    reason: string | null;
    /** While armed: seconds until the PC sleeps if nothing changes. */
    remaining_seconds: number | null;
  };
  /** Unix seconds; `error` when Windows refused to sleep. */
  last_sleep: { at: number; error: string | null } | null;
  fix: {
    running: boolean;
    /** The changes a fix would make; null when the adapters could not be read (`error`). */
    changes: string[] | null;
    /** Adapters that need a change the host can't make, and why. */
    skipped: string[] | null;
    error: string | null;
    /** Fast Startup is active: turning it off is its own choice. */
    fast_startup_off_offered: boolean;
    last: {
      at: number;
      changes?: string[];
      restarted?: string[];
      after_reboot?: string[];
      errors: string[];
    } | null;
  };
}
export interface PowerFixResult {
  status: true;
  /** Empty when there was nothing to change (200); applied about a second later otherwise (202). */
  changes: string[];
  skipped: string[];
}

// ---------- endpoints ----------

export const api = {
  auth: {
    status: () => get<AuthStatus>('/api/auth/status'),
    async login(username: string, password: string, rememberMe: boolean) {
      await refreshCsrf();
      const session = await post<IssuedSession>('/api/auth/login', { username, password, remember_me: rememberMe });
      csrfToken = session.csrf_token;
      return session;
    },
    logout: () => post<Ok>('/api/auth/logout'),
    async resume() {
      await refreshCsrf();
    },
    sessions: () => get<{ status: true; sessions: WebSession[] }>('/api/auth/sessions'),
    revokeSession: (id: string) => json<Ok>('DELETE', `/api/auth/sessions/${encodeURIComponent(id)}`),
    setPassword: (body: {
      currentUsername?: string;
      currentPassword?: string;
      newUsername: string;
      newPassword: string;
      confirmNewPassword: string;
    }) => post<Ok>('/api/password', body),
  },
  metadata: () => get<Metadata>('/api/metadata'),
  /** List a folder of the host; an empty path lists the drives. */
  browse: (path: string, type: BrowseType) =>
    get<BrowseListing>(`/api/browse?path=${encodeURIComponent(path)}&type=${type}`, ACTION_TIMEOUT_MS),
  config: {
    get: () => get<Config>('/api/config'),
    /** Merge keys; null or "" resets a key to its default. */
    patch: (values: Config) => json<{ status: true; restart_required: boolean; warning: string | null }>('PATCH', '/api/config', values),
  },
  apps: {
    list: () => get<AppsDocument>('/api/apps'),
    /** Replaces the whole record; start from the object returned by list(). */
    save: (app: App) => post<{ status: true; uuid: string }>('/api/apps', app),
    remove: (uuid: string) => json<Ok>('DELETE', `/api/apps/${encodeURIComponent(uuid)}`),
    reorder: (order: string[]) => post<Ok>('/api/apps/reorder', { order }),
    launch: (uuid: string) => post<Ok>('/api/apps/launch', { uuid }, SLOW_TIMEOUT_MS),
    close: () => post<Ok>('/api/apps/close', {}, SLOW_TIMEOUT_MS),
    coverUrl: (uuid: string, version = '') => `/api/apps/${encodeURIComponent(uuid)}/cover${version ? `?v=${version}` : ''}`,
    /** Store a cover from an https PNG URL (IGDB) or a PNG as base64. */
    uploadCover: (key: string, source: { url: string } | { data: string }) =>
      post<{ status: true; path: string }>('/api/covers/upload', { key, ...source }, ACTION_TIMEOUT_MS),
  },
  library: {
    discovery: () => get<LibraryDiscovery>('/api/library/discovery', ACTION_TIMEOUT_MS),
    import: (games: { source: LibrarySource; id: string }[]) =>
      post<{ status: true; results: LibraryImportResult[] }>('/api/library/import', { games }, ACTION_TIMEOUT_MS),
  },
  steam: {
    status: () => get<SteamStatus>('/api/steam/status'),
    sync: () =>
      post<{ status: true; changed: boolean; game_count: number; importable_game_count: number }>(
        '/api/steam/force_sync',
        {},
        SLOW_TIMEOUT_MS,
      ),
  },
  playnite: {
    status: () => get<PlayniteStatus>('/api/playnite/status'),
    sync: () => post<{ status: true; changed: boolean; game_count: number }>('/api/playnite/force_sync', {}, SLOW_TIMEOUT_MS),
    install: () => post<{ status: true; path: string; restart_required: boolean }>('/api/playnite/install', {}, ACTION_TIMEOUT_MS),
    /** Make a saved cover (by its key) the game's cover in Playnite. */
    setCover: (playniteId: string, coverKey: string) =>
      post<{ status: true; path: string }>('/api/playnite/cover', { playnite_id: playniteId, cover_key: coverKey }, SLOW_TIMEOUT_MS),
    launch: () => post<Ok>('/api/playnite/launch', {}, ACTION_TIMEOUT_MS),
  },
  clients: {
    /** `sleep_unavailable`: why this PC offers no Sleep PC, null when it does. */
    list: () => get<{ status: true; clients: Client[]; sleep_unavailable?: string | null }>('/api/clients/list'),
    pending: () => get<{ status: true; requests: PendingPairing[] }>('/api/clients/pending'),
    update: (client: Partial<Client> & { uuid: string }) => {
      const { connected: _connected, ...rest } = client;
      return post<Ok>('/api/clients/update', rest, ACTION_TIMEOUT_MS);
    },
    unpair: (uuid: string) => post<Ok>('/api/clients/unpair', { uuid }, ACTION_TIMEOUT_MS),
    unpairAll: () => post<Ok>('/api/clients/unpair-all', {}, ACTION_TIMEOUT_MS),
    disconnect: (uuid: string) => post<Ok>('/api/clients/disconnect', { uuid }, ACTION_TIMEOUT_MS),
    resetNetworkHistory: (uuid: string) => post<Ok>('/api/clients/reset-network-history', { uuid }, ACTION_TIMEOUT_MS),
    pin: (pin: string, uniqueid?: string, name?: string) => post<Ok>('/api/pin', { pin, uniqueid, name }),
    otp: (passphrase: string, deviceName: string) =>
      post<{ status: true; otp: string; ip: string; name: string }>('/api/otp', { passphrase, deviceName }),
    hdrProfiles: () => get<{ status: true; profiles: { filename: string }[] }>('/api/clients/hdr-profiles'),
  },
  sessions: {
    status: () => get<SessionStatus>('/api/session/status'),
    streams: () => get<{ status: true; sessions: StreamSession[] }>('/api/rtsp/sessions'),
    console: () => get<{ status: true; recovery: ConsoleRecovery }>('/api/session/console'),
    returnToConsole: (recovery: ConsoleRecovery) => post<Ok>('/api/session/console', {
      session_id: recovery.session_id,
      console_session_id: recovery.console_session_id,
    }, ACTION_TIMEOUT_MS),
  },
  displays: {
    devices: () => get<DisplayDevice[]>('/api/display-devices'),
    goldenStatus: () => get<GoldenStatus>('/api/display/golden_status?compare_current=1'),
    exportGolden: () => post<Ok>('/api/display/export_golden'),
    restoreGolden: () => post<Ok>('/api/display/restore_golden', {}, ACTION_TIMEOUT_MS),
    deleteGolden: () => json<Ok>('DELETE', '/api/display/golden'),
    disconnectVirtual: () => post<Ok>('/api/display/terminate_virtual', {}, ACTION_TIMEOUT_MS),
    resetPersistence: () => post<Ok>('/api/reset-display-device-persistence', {}, ACTION_TIMEOUT_MS),
  },
  health: {
    limiter: () => get<LimiterStatus>('/api/frame-limiter/status'),
    vulkanLayer: () => get<VulkanLayerStatus>('/api/health/vulkan-hdr-layer'),
    registerVulkanLayer: () => post<VulkanLayerStatus>('/api/health/vulkan-hdr-layer/register'),
    crash: () => get<CrashStatus>('/api/health/crashdump'),
    dismissCrash: (filename: string, captured_at: string) =>
      post<Ok>('/api/health/crashdump/dismiss', { filename, captured_at }),
  },
  logs: {
    /** Bytes after `offset`, at most `max`; offset -1 means the last `max` bytes. */
    read: (offset: number, max = 256 * 1024) => get<LogChunk>(`/api/logs/tail?offset=${offset}&max=${max}`),
    /** Known failures in the log's last 8 MiB, explained by the host's local rules. */
    analysis: () => get<LogAnalysis>('/api/logs/analysis'),
    downloadUrl: '/api/logs/export',
    supportBundleUrl: '/api/logs/export_crash',
  },
  updates: {
    state: () => get<UpdatesState>('/api/updates'),
    check: () => post<Ok>('/api/updates/check'),
    install: () => post<Ok>('/api/updates/install'),
    installNow: () => post<Ok>('/api/updates/install_now'),
    cancel: () => post<Ok>('/api/updates/cancel'),
  },
  power: {
    /** `refresh` reads the adapters again instead of the host's last reading. */
    status: (refresh = false) => get<PowerStatus>(`/api/power${refresh ? '?refresh=1' : ''}`, ACTION_TIMEOUT_MS),
    /** Applies exactly `changes`, as `status` listed them; 409 when they changed since. */
    fixWake: (changes: string[], fastStartupOff: boolean) =>
      post<PowerFixResult>('/api/power/wake/fix', { changes, fast_startup_off: fastStartupOff }, ACTION_TIMEOUT_MS),
  },
  tokens: {
    list: () => get<{ status: true; tokens: ApiToken[] }>('/api/tokens'),
    routes: () => get<{ status: true; routes: TokenScope[] }>('/api/token/routes'),
    create: (scopes: TokenScope[]) => post<{ status: true; token: string }>('/api/token', { scopes }),
    revoke: (hash: string) => json<Ok>('DELETE', `/api/token/${encodeURIComponent(hash)}`),
  },
  host: {
    restart: () => post<Ok>('/api/restart'),
    quit: () => post<Ok>('/api/quit'),
  },
};

export const CODEC_NAMES = ['H.264', 'HEVC', 'AV1', 'PyroWave'] as const;
