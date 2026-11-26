// Typed client for the Rubylight admin API (/api on the console port).
// Sessions live in HttpOnly cookies; writes carry the session's CSRF token.
import { polled } from './format';
import type {
  ApiToken,
  App,
  AppsDocument,
  AuthStatus,
  BackupInspection,
  Client,
  Config,
  IssuedSession,
  LibraryDiscovery,
  LibraryImportResult,
  LibrarySource,
  Metadata,
  NvidiaComponent,
  Ok,
  PendingPairing,
  PlayniteStatus,
  RadeonPanel,
  SteamStatus,
  TokenScope,
  UsbAttachment,
  WebSession,
} from './api/types-core';
import type {
  ChildSessionStatus,
  ConsoleRecovery,
  CoopInstall,
  CoopSessions,
  CoopStatus,
  DisplayDevice,
  GameSettingsStatus,
  GoldenStatus,
  LimiterStatus,
  NeuralStatus,
  QuickSuspendStatus,
  SessionHistory,
  SessionStatus,
  StreamSession,
} from './api/types-sessions';
import type {
  BrowseListing,
  BrowseType,
  CrashStatus,
  CreatedInvite,
  GuestInvites,
  GuestPermission,
  GuestReach,
  GuestsStatus,
  InternetPlayStatus,
  InviteCreated,
  LogAnalysis,
  LogChunk,
  PairingAccess,
  PairingCode,
  PowerFixResult,
  PowerStatus,
  SessionGuestsStatus,
  UpdatesState,
  VulkanLayerStatus,
  WebhookList,
  WebhookSave,
  WebhookStatus,
} from './api/types-network';

export * from './api/types-core';
export * from './api/types-sessions';
export * from './api/types-network';

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

// One refresh at a time across calls and tabs: the host rotates the refresh token
// (with a short grace period), so concurrent refreshes would otherwise race and sign out.
let refreshing: Promise<boolean> | null = null;
function refreshSession(): Promise<boolean> {
  refreshing ??= (async () => {
    // The refresh cookie goes with every request to the console, so the host wants the page's
    // CSRF token with it: the session's, or after a reload the anonymous one /api/csrf-token gives.
    const attempt = () =>
      exchange('/api/auth/refresh', { method: 'POST', headers: { 'X-CSRF-Token': csrfToken } }, REQUEST_TIMEOUT_MS);
    const run = async (): Promise<boolean> => {
      if (!csrfToken) await refreshCsrf();
      let { response, text } = await attempt();
      if (response.status === 400 && text.includes('CSRF token required')) {
        await refreshCsrf();
        ({ response, text } = await attempt());
      }
      if (!response.ok) return false;
      let body: IssuedSession;
      try {
        body = JSON.parse(text) as IssuedSession;
      } catch {
        return false;
      }
      csrfToken = body.csrf_token;
      return true;
    };
    const locks = typeof navigator !== 'undefined' ? navigator.locks : undefined;
    return locks ? locks.request('rubylight-auth-refresh', run) : run();
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
  if (
    retried ||
    path === '/api/auth/login' ||
    path === '/api/auth/refresh' ||
    path === '/api/auth/status'
  ) {
    return reply;
  }
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

/**
 * A POST whose body or answer is a file (a settings backup): the answer as a Blob. Retries once
 * after a refreshed session or CSRF token, as `send` does.
 */
async function binary(path: string, body: BodyInit, contentType: string, timeout: number, retried = false): Promise<Blob> {
  if (!csrfToken) await refreshCsrf();
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timeout);
  let response: Response;
  let blob: Blob;
  try {
    response = await fetch(path, {
      method: 'POST',
      credentials: 'same-origin',
      signal: controller.signal,
      headers: { 'X-CSRF-Token': csrfToken, 'Content-Type': contentType },
      body,
    });
    blob = await response.blob();
  } catch (error) {
    if (controller.signal.aborted) throw new TimeoutError(timeout);
    throw error;
  } finally {
    clearTimeout(timer);
  }
  if (response.ok) return blob;
  const text = await blob.text();
  if (!retried && response.status === 401) {
    if (await refreshSession()) return binary(path, body, contentType, timeout, true);
    onSignedOut?.();
  }
  if (!retried && response.status === 400 && text.includes('CSRF token required')) {
    await refreshCsrf();
    return binary(path, body, contentType, timeout, true);
  }
  let message = text || `${response.status} ${response.statusText}`;
  try {
    message = (JSON.parse(text) as { error?: string }).error ?? message;
  } catch {
    // Some errors are plain text.
  }
  throw new ApiError(response.status, message);
}

const get = <T>(path: string, timeout = REQUEST_TIMEOUT_MS) => json<T>('GET', path, undefined, timeout);
const post = <T>(path: string, body: unknown = {}, timeout = REQUEST_TIMEOUT_MS) => json<T>('POST', path, body, timeout);

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
  /** Hide a host warning (`Metadata.warnings`) until its text changes. */
  dismissWarning: (code: string) => post<Ok>('/api/warnings/dismiss', { code }),
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
    /**
     * Several apps in one save; uuids already gone are skipped. Taking Neural Rendering out of
     * a game's folder can take minutes; an app whose mod stays is kept and named in `kept`.
     */
    removeMany: (uuids: string[]) =>
      post<{ status: true; deleted: number; kept: { name: string; error: string }[] }>(
        '/api/apps/delete',
        { uuids },
        SLOW_TIMEOUT_MS,
      ),
    reorder: (order: string[]) => post<Ok>('/api/apps/reorder', { order }),
    launch: (uuid: string) => post<Ok>('/api/apps/launch', { uuid }, SLOW_TIMEOUT_MS),
    close: () => post<Ok>('/api/apps/close', {}, SLOW_TIMEOUT_MS),
    coverUrl: (uuid: string, version = '') => `/api/apps/${encodeURIComponent(uuid)}/cover${version ? `?v=${version}` : ''}`,
    /** Store a cover from an https PNG URL (IGDB) or a PNG as base64. */
    uploadCover: (key: string, source: { url: string } | { data: string }) =>
      post<{ status: true; path: string }>('/api/covers/upload', { key, ...source }, ACTION_TIMEOUT_MS),
  },
  /** The Radeon settings this PC's GPU supports, for a game's Radeon controls. */
  radeon: () => get<RadeonPanel>('/api/radeon'),
  library: {
    discovery: () => get<LibraryDiscovery>('/api/library/discovery', ACTION_TIMEOUT_MS),
    import: (games: { source: LibrarySource; id: string }[]) =>
      post<{ status: true; results: LibraryImportResult[] }>('/api/library/import', { games }, ACTION_TIMEOUT_MS),
    /** The launcher sync now: every installed game of the launchers that are on. */
    sync: () =>
      post<{ status: true; added: number; removed: number; playnite: boolean }>('/api/library/sync', {}, SLOW_TIMEOUT_MS),
    /** Removed games (`launcher-dismissed` keys) the syncs may add again; sync next. */
    restore: (games: string[]) => post<{ status: true; restored: number }>('/api/library/restore', { games }),
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
    /** `access` is what a Moonlight device may do once paired; WebRTC requests take none. */
    pin: (pin: string, uniqueid?: string, name?: string, access?: PairingAccess) => post<Ok>('/api/pin', { pin, uniqueid, name, access }),
    otp: (passphrase: string, deviceName: string, access?: PairingAccess) =>
      post<{ status: true; otp: string; ip: string; name: string }>('/api/otp', { passphrase, deviceName, access }),
    hdrProfiles: () => get<{ status: true; profiles: { filename: string }[] }>('/api/clients/hdr-profiles'),
    /** The live code while it has a minute left, else a new one; `access` applies to it either way. */
    pairingQr: (address = '', access?: PairingAccess) =>
      post<{ status: true; codes: PairingCode[]; name: string; expires_in_seconds: number }>('/api/pairing/qr', { address, access }),
  },
  webhooks: {
    list: () => get<WebhookList>('/api/webhooks'),
    /** The secret comes back only when the hook is new or its secret was replaced. */
    save: (hook: WebhookSave) => post<{ status: true; secret: string | null }>('/api/webhooks', hook, ACTION_TIMEOUT_MS),
    remove: (url: string) => post<Ok>('/api/webhooks/delete', { url }, ACTION_TIMEOUT_MS),
    test: (url: string) => post<{ status: true; result: WebhookStatus }>('/api/webhooks/test', { url }, ACTION_TIMEOUT_MS),
  },
  sessions: {
    status: () => get<SessionStatus>('/api/session/status'),
    streams: () => get<{ status: true; sessions: StreamSession[] }>('/api/rtsp/sessions'),
    /** The ended streams, newest first, within the host's count and age caps. */
    history: () => get<SessionHistory>('/api/sessions/history'),
    console: () => get<{ status: true; recovery: ConsoleRecovery }>('/api/session/console'),
    returnToConsole: (recovery: ConsoleRecovery) => post<Ok>('/api/session/console', {
      session_id: recovery.session_id,
      console_session_id: recovery.console_session_id,
    }, ACTION_TIMEOUT_MS),
    /** Quick Suspend: the running game's paused state; a refusal is an answer, never an error. */
    suspendStatus: () => get<QuickSuspendStatus>('/api/session/suspend'),
    suspend: () => post<QuickSuspendStatus>('/api/session/suspend', {}, ACTION_TIMEOUT_MS),
    resume: () => post<QuickSuspendStatus>('/api/session/resume', {}, ACTION_TIMEOUT_MS),
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
  childSession: {
    status: () => get<ChildSessionStatus>('/api/child-session'),
    /** Runs the host's start-up restore again; the answer is the new status. */
    restore: () => post<ChildSessionStatus>('/api/child-session/restore', {}, ACTION_TIMEOUT_MS),
  },
  guests: {
    status: () => get<GuestsStatus>('/api/guests'),
    /** This PC's LAN addresses, for a console opened on the PC itself. */
    addresses: () => get<{ addresses: string[] }>('/api/guests/addresses'),
    /** A link lasting `seconds` (five minutes to seven days) at `address`, which guests' media is offered at too;
     * `internet` keeps the guest ports mapped on the router while it works. */
    invite: (permission: GuestPermission, seconds: number, address: string, internet = false) =>
      post<CreatedInvite>('/api/guests/invite', { permission, seconds, address, internet }),
    /** What the router did with the guest ports, and the address a link for the internet carries. */
    internet: () => get<GuestReach>('/api/guests/internet'),
    /** Asks the router for the guest ports now, for two minutes or until a link holds them. */
    prepareInternet: () => post<GuestReach>('/api/guests/internet', {}),
    /** The link opens nothing any more; whoever joined with it is disconnected. */
    revoke: (id: string) => post<Record<string, never>>('/api/guests/revoke', { id }),
    /** Disconnects the guest; its link still works. */
    kick: (id: number) => post<Record<string, never>>('/api/guests/kick', { id }),
  },
  sessionGuests: {
    status: () => get<SessionGuestsStatus>('/api/session-guests'),
    /** Lets a waiting device in, with `permission`, for `seconds` (300 to 604800) or until the game ends (null). */
    approve: (id: number, permission: GuestPermission, seconds: number | null = null) =>
      post<SessionGuestsStatus>('/api/session-guests/approve', { id, permission, seconds }),
    deny: (id: number) => post<SessionGuestsStatus>('/api/session-guests/deny', { id }),
    /** Ends the guest's stream; it may ask to join again. */
    kick: (id: number) => post<SessionGuestsStatus>('/api/session-guests/kick', { id }),
    /** Ends the guest's stream and refuses its device until the game ends. */
    revoke: (id: number) => post<SessionGuestsStatus>('/api/session-guests/revoke', { id }),
    permission: (id: number, permission: GuestPermission) =>
      post<SessionGuestsStatus>('/api/session-guests/permission', { id, permission }),
    /** Hands the keyboard and mouse to guest `id`, or back to the device playing with null. */
    keyboardMouse: (id: number | null) => post<SessionGuestsStatus>('/api/session-guests/keyboard-mouse', { id }),
    invites: () => get<GuestInvites>('/api/session-guests/invites'),
    /** A link that pairs one friend's Nightjar over the internet, for `seconds` (300 to 604800). */
    invite: (permission: GuestPermission, seconds: number) =>
      post<InviteCreated>('/api/session-guests/invites', { permission, seconds }),
    /** Takes back a link nobody used yet. */
    revokeInvite: (id: number) => post<GuestInvites>('/api/session-guests/invites/revoke', { id }),
    /** Removes a device paired from an invite. */
    removeInvited: (uuid: string) => post<GuestInvites>('/api/session-guests/invites/revoke', { uuid }),
  },
  coop: {
    status: () => get<CoopStatus>('/api/coop'),
    /** The sessions alone, for Home's poll: no app's co-op status or install is read. */
    sessions: () => get<CoopSessions>('/api/coop/sessions'),
    /** Checks the folder Nucleus Co-op was unpacked into and keeps it; refused with the reason. Empty forgets it. */
    folder: (path: string) => post<CoopInstall>('/api/coop/folder', { path }),
    /** Opens Nucleus Co-op's window on the PC's screen, for adding a handler. */
    open: () => post<Record<string, never>>('/api/coop/open', {}, ACTION_TIMEOUT_MS),
    /** Closes the window `open` opened; `closed` is false when it was already gone. Never another Nucleus Co-op. */
    close: () => post<{ closed: boolean }>('/api/coop/close', {}, ACTION_TIMEOUT_MS),
    /** Frees the slot; the device's stream ends. */
    kick: (session: number, slot: number) => post<{ device: string }>('/api/coop/kick', { session, slot }, ACTION_TIMEOUT_MS),
    /** Stops Nucleus Co-op and every instance, and ends each player's stream. */
    end: (session: number) => post<Record<string, never>>('/api/coop/end', { session }, ACTION_TIMEOUT_MS),
  },
  neural: {
    status: (uuid: string) => get<NeuralStatus>(`/api/neural?uuid=${encodeURIComponent(uuid)}`),
    /** Records the notice's acceptance; the host refuses a download before it. */
    accept: () => post<Ok>('/api/neural/accept', {}),
    /** Downloads unless cached and installs; progress and a refusal show in `status`. */
    enable: (uuid: string, exe?: string) => post<Ok>('/api/neural/enable', exe ? { uuid, exe } : { uuid }),
    /** Removes exactly what was installed. */
    disable: (uuid: string, earlier = false) => post<Ok>('/api/neural/disable', earlier ? { uuid, earlier } : { uuid }),
  },
  gameSettings: {
    status: (uuid: string) => get<GameSettingsStatus>(`/api/game-settings?uuid=${encodeURIComponent(uuid)}`),
    /** Puts a waiting swap's files back without waiting for the process it waits for. */
    release: (id: string) => post<Ok>('/api/game-settings/release', { id }, ACTION_TIMEOUT_MS),
  },
  nvidiaComponent: {
    status: () => get<NvidiaComponent>('/api/nvidia-component'),
    /** Starts the download from this version's release; `status` shows its progress. */
    install: () => post<Ok>('/api/nvidia-component/install', {}),
    remove: () => post<Ok>('/api/nvidia-component/remove', {}),
  },
  health: {
    limiter: () => get<LimiterStatus>('/api/frame-limiter/status'),
    internetPlay: () => get<InternetPlayStatus>('/api/internet-play'),
    vulkanLayer: () => get<VulkanLayerStatus>('/api/health/vulkan-hdr-layer'),
    registerVulkanLayer: () => post<VulkanLayerStatus>('/api/health/vulkan-hdr-layer/register'),
    crash: () => get<CrashStatus>('/api/health/crashdump'),
    dismissCrash: (filename: string, captured_at: string) =>
      post<Ok>('/api/health/crashdump/dismiss', { filename, captured_at }),
  },
  backup: {
    /** The settings as a ZIP archive; `secrets` adds the pairing identity and credentials. */
    export: (secrets: boolean) =>
      binary('/api/backup/export', JSON.stringify({ secrets }), 'application/json', ACTION_TIMEOUT_MS),
    /** Checks an archive and keeps it on the host for `restore`. */
    inspect: async (file: Blob) =>
      JSON.parse(await (await binary('/api/backup/inspect', file, 'application/zip', SLOW_TIMEOUT_MS)).text()) as BackupInspection,
    /** Saves the current settings, applies the checked archive and restarts the host. */
    restore: (token: string) => post<Ok & { saved?: string }>('/api/backup/restore', { token }, ACTION_TIMEOUT_MS),
  },
  logs: {
    /** Bytes after `offset`, at most `max`; offset -1 means the last `max` bytes. */
    read: (offset: number, max = 256 * 1024, head?: string) =>
      get<LogChunk>(`/api/logs/tail?offset=${offset}&max=${max}${head ? `&head=${encodeURIComponent(head)}` : ''}`),
    /** Known failures in the log's last 8 MiB, explained by the host's local rules. */
    analysis: () => get<LogAnalysis>('/api/logs/analysis'),
    downloadUrl: '/api/logs/export',
    supportBundleUrl: '/api/logs/export_crash',
    /** The support bundle with the newest crash memory dump, which is left out unless asked for. */
    supportBundleWithDumpUrl: '/api/logs/export_crash?include_dump=1',
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
  usb: {
    list: () => get<{ status: true; devices: UsbAttachment[] }>('/api/usb'),
    // The host runs usbip.exe for a device the driver kept, so this can take a while.
    detach: (id: number) => post<Ok>('/api/usb/detach', { id }, ACTION_TIMEOUT_MS),
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
