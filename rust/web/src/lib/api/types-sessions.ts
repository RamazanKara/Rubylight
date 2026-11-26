// Streaming, performance, display, co-op, game settings and neural API types.

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
/** The host stages the stutter report can name. */
export type StutterStageId = 'capture' | 'convert' | 'encode' | 'packetize' | 'send';
/** One host stage's share of the time this stream's pictures lost against its own usual path. */
export interface StutterStage {
  id: StutterStageId;
  lost_ms: number;
  /** Of the report's `lost_ms`, in whole percent. */
  percent: number;
}
/** Where this stream's pictures lost time, from the host's present to its last packet. */
export interface StutterReport {
  /** Five-second windows the report covers. */
  windows: number;
  /** New pictures measured; static repeats have no present and are not counted. */
  pictures: number;
  /** Pictures at least half a frame (or a millisecond) later than this stream's usual path. */
  late_pictures: number;
  lost_ms: number;
  /** The stage with the most lost time, or null when none stands out. */
  dominant: StutterStageId | null;
  /** The stages that took time, most first. */
  stages: StutterStage[];
  /** Stages the host cannot see: the game's own intervals, the network and the device. */
  not_measured: ('game' | 'network' | 'decode' | 'present')[];
}
export type Role = 'stream' | 'remote_monitor' | 'input_only';
export interface StreamSession {
  /** The guest id when this stream joined another device's game as a guest (`/api/session-guests`). */
  guest?: number | null;
  encoder: string;
  warnings: { code: string; message: string }[];
  uuid: string;
  device_name: string;
  width: number;
  height: number;
  /** The size the PC's display keeps; differs from `width`/`height` when Maximum stream resolution caps the coded picture. */
  display_width?: number;
  display_height?: number;
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
    /** The highest rate that ran clean, or 0 while the path has shown no edge; probes stay under it. */
    learned_kbps: number;
    /** The PC's own socket refused video within the last 5 s: probes stop and this is not a network limit. */
    host_limited: boolean;
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
  /** PyroWave streams: text and textures may lose detail below this bitrate. */
  pyrowave_minimum_kbps: number | null;
  /** A clean-picture target from synthetic scenes, not a quality guarantee. */
  pyrowave_recommended_kbps: number | null;
  /** Frames per second at each host stage over the last `window_ms` (at least a second, between console refreshes), in millihertz; null for a stage the host can't count. Absent until the first window. */
  pipeline_rates?: {
    window_ms: number;
    source_mhz: number | null;
    capture_mhz: number | null;
    submit_mhz: number | null;
    output_mhz: number | null;
    repeat_mhz: number | null;
  } | null;
  audio_channels: number;
  state: 'RUNNING' | 'STOPPING';
  frames_sent: number;
  bytes_sent: number;
  idr_requests: number;
  performance: Performance;
  /** Where the stream's new pictures lost time so far, from the host's stage stamps. */
  stutter?: StutterReport;
  uptime_seconds: number;
  role: Role;
}
export interface ConsoleRecovery {
  session_id: number;
  console_session_id: number;
  can_reattach: boolean;
  reason: string;
}

/** One stream that ended, from `GET /api/sessions/history` (newest first). */
export interface SessionHistoryEntry {
  /** The stream's own id. */
  id: string;
  /** The device's paired uuid and its name at the time. */
  device: string;
  device_name: string;
  app_id: number;
  app: string;
  role: Role;
  transport: 'moonlight' | 'webrtc';
  /** `h264`, `hevc`, `av1` or `pyrowave`. */
  codec: string;
  /** The picture the stream ran at, after any mid-stream mode switch; 0 when it never started. */
  width: number;
  height: number;
  fps: number;
  hdr: boolean;
  /** The encoder's bitrate when the stream ended; 0 when it never started. */
  bitrate_kbps: number;
  /** The bitrate the stream asked for; 0 when it never started or for an entry saved by an older host. */
  requested_bitrate_kbps: number;
  /** What the video actually sent on average; audio is not counted. */
  mean_bitrate_kbps: number;
  mean_fps: number;
  /** The frame rate 5% of the sampled seconds ran at or below (the 95th percentile of frame time); 0 until 20 seconds were sampled. */
  p95_fps: number;
  /** The client's reported missing video packets, as a share of what the host sent; null when it reported no FEC blocks. */
  loss_percent: number | null;
  end_reason: 'ended' | 'failed' | 'start_failed' | 'interrupted' | string;
  /** The host's words for a failure, when it failed. An interrupted session ends at the host's last heartbeat. */
  end_detail: string;
  /** `encoder`, `capture`, `network` or `driver` when the log analysis knows the failure. */
  end_category: 'encoder' | 'capture' | 'network' | 'driver' | null;
  started_unix_ms: number;
  ended_unix_ms: number;
  duration_ms: number;
}
export interface SessionHistory {
  status: true;
  sessions: SessionHistoryEntry[];
  /** How many sessions are kept at most; the page says so. */
  limit: number;
  /** How many days a session is kept; the page says so. */
  days: number;
  /** The byte cap that can trim below `limit` sooner; the page says "up to". */
  bytes: number;
}

export interface SessionStatus {
  status: true;
  activeSessions: number;
  appRunning: boolean;
  appName: string;
  paused: boolean;
  running: boolean;
  /** `uuid` is the library app that runs; null for one that is no longer in the library. */
  app: { name: string; id: number; uuid?: string | null } | null;
}
export interface DisplayDevice {
  device_id: string;
  display_name: string;
  friendly_name: string;
  hdr_supported: boolean;
  hdr_enabled: boolean;
  primary: boolean;
  /** Where it sits on the Windows desktop and its mode; absent when Windows cannot report them. */
  x?: number;
  y?: number;
  width?: number;
  height?: number;
  /** Hz. */
  refresh?: number;
  /** Windows' scale in percent. */
  scale?: number;
  /** A Rubylight virtual display. */
  virtual?: boolean;
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
/** `GET /api/child-session`: whether Windows settings the removed child-session feature's setup changed are still changed, and why the last restore failed. */
export interface ChildSessionStatus {
  pending: boolean;
  problem: string | null;
}
export type CoopPhase = 'starting' | 'running' | 'ended';
/** How a Nucleus Co-op session ended; `detail` says what stopped for `crashed` and `failed`. */
export interface CoopEnd {
  reason: 'ended' | 'empty' | 'crashed' | 'failed';
  detail: string | null;
}
export interface CoopPlayer {
  slot: number;
  /** The paired device's uuid; null for a free slot. */
  device: string | null;
  name: string | null;
  /** The device's stream is gone; it keeps the slot for a while. */
  away: boolean;
  /** Seconds an away device has left to come back (older hosts leave it out). */
  rejoin_in?: number | null;
}
export interface CoopSession {
  id: number;
  kind: 'coop' | 'background';
  app_id: number;
  title: string;
  game: string;
  phase: CoopPhase;
  end: CoopEnd | null;
  players: CoopPlayer[];
  /** Devices removed from this session: they can't rejoin it (older hosts leave it out). */
  kicked?: string[];
}
/**
 * One app's co-op on the host: `unknown` before Nucleus Co-op is set up (or while its handlers don't load),
 * `ready` with a handler and the added game, `launcher` for an app a launcher's link starts (no handler can
 * match it), `taken` when another app already lists under its co-op entry's name, `anti_cheat` for a game that
 * ships anti-cheat (Rubylight never hooks it), `refused` for one Rubylight's own handler never starts (a game on
 * a network share, Windows' own program, a missing .exe, a game that needs administrator rights on an account
 * Windows doesn't elevate), otherwise why not, in the host's words (`reason`).
 */
export type CoopGameStatus =
  | 'unknown'
  | 'ready'
  | 'no_handler'
  | 'not_added'
  | 'no_executable'
  | 'launcher'
  | 'taken'
  | 'anti_cheat'
  | 'refused';
export interface CoopGame {
  /** The app's uuid. */
  app: string;
  name: string;
  status: CoopGameStatus;
  reason: string | null;
  /** The handler's name when ready. */
  handler: string | null;
  /** The most players a session seats when ready. */
  players: number | null;
  /**
   * How a background stream of it runs Nucleus Co-op with Rubylight's own handler: as the signed-in user
   * or elevated. Null when the hub handler runs it (always elevated) or the game is checked at launch;
   * older hosts leave it out.
   */
  runs?: 'user' | 'elevated' | null;
  /** The co-op entry's name in the library: `<name> (Co-op)`. */
  entry: string;
}
/** What the Neural Rendering switch is doing for one app, or how it ended. */
export interface NeuralJob {
  /** `update` is the check a launch runs on a game that has it on. */
  action: 'install' | 'remove' | 'update';
  phase: 'download' | 'place' | 'remove' | 'done' | 'failed';
  /** Bytes so far and in all: the download, then the copy into the game's folder. */
  done: number;
  total: number;
  error?: string;
  /** The game's program could not be found: the user picks it. */
  choose_exe: boolean;
  /** Files that changed since they were placed and stayed in the game's folder. */
  kept: string[];
}
/** `GET /api/game-settings?uuid=`: the saved app's game settings files, per paired device. */
export interface GameSettingsStatus {
  /** As saved. */
  files: string[];
  /** Each paired device and the files it has a saved copy of. */
  devices: { uuid: string; name: string; saved: string[] }[];
  /** Swaps still in place: the running game's, or one the host could not put back yet. */
  pending: { id: string; device: string; deviceName: string; running: boolean; error: string | null }[];
  /** Where the host keeps the copies and the PC's originals. */
  folder: string;
}
/** `GET /api/neural?uuid=`: the game page's Neural Rendering switch. */
export interface NeuralStatus {
  /** The one-time notice before the first download. */
  accepted: boolean;
  /** Whether this PC's GPU can run frame generation; `reason` says why not. */
  available: boolean;
  /** The GPU it runs on: the one Rubylight streams with, else another of the PC's. */
  gpu: string | null;
  /** When `gpu` is not the GPU Rubylight streams with: that it runs when the game renders there. */
  gpu_note?: string | null;
  reason: string | null;
  found: boolean;
  enabled: boolean;
  /** The program frame generation sits beside, once it is in. */
  exe: string | null;
  release: string | null;
  /** `full`: the host's own package; `addon`: added to an upscaler mod the user installed. */
  mode: 'full' | 'addon' | null;
  /**
   * The game's own check before any switch, while it is off: `why` is `anti_cheat`, `no_frame_generation`, `game`
   * (refused, `reason` says why) or `choose_program` (Rubylight can't tell which program is the game).
   */
  game?: { why: string; reason: string } | null;
  /** Rubylight 3.0's neural upscaling, still in the game's folder until the user removes it. */
  earlier_upscaling: { exe: string | null; release: string | null } | null;
  job: NeuralJob | null;
}
/**
 * Where Nucleus Co-op stands: `installed` is an earlier Rubylight's own install, `configured` the folder the user
 * chose. Rubylight downloads nothing; the user unpacks the release from its page and chooses its folder.
 */
export interface CoopInstall {
  state: 'installed' | 'missing' | 'configured' | 'failed';
  path: string | null;
  /** The folder chosen (`nucleus_path`), empty when none is. */
  chosen?: string;
  version: string | null;
  /**
   * Why the chosen folder can't run: no NucleusCoop.exe, another release, a folder others can change, or files
   * Windows marks as downloaded from the internet.
   */
  problem?: 'missing' | 'version' | 'untrusted' | 'blocked' | null;
  error: string | null;
  /**
   * Why the copy an earlier Rubylight installed runs instead of the chosen folder (it isn't the pinned release);
   * null when it doesn't, and older hosts leave it out.
   */
  notice?: string | null;
  /** The release to unpack and the page it downloads from (older hosts leave it out). */
  release?: { version: string; page: string };
}

/** `GET /api/coop`: background streaming and co-op through Nucleus Co-op. */
export interface CoopStatus {
  mode: {
    setting: string;
    /** Nucleus Co-op is the only background mode. */
    mode: 'nucleus';
    label: string;
    reason: string;
  };
  install: CoopInstall;
  available: boolean;
  /** Why this host runs no Nucleus Co-op sessions; null when it does. */
  reason: string | null;
  /**
   * Whether Windows gives the account signed in on the PC an elevated token, which hub handlers, co-op and
   * games that need administrator rights run with. Null when the host can't tell; older hosts leave it out.
   */
  elevates?: boolean | null;
  /**
   * An administrator whom Windows' Administrator protection gives no elevated token Rubylight can use (older
   * hosts leave it out).
   */
  admin_protection?: boolean;
  /** Why Open Nucleus Co-op on this PC would be refused (an account Windows doesn't elevate); null otherwise. */
  open_refusal?: string | null;
  /** The Nucleus Co-op window the console opened is still open (older hosts leave it out). */
  opened?: boolean;
  /** Every app that starts an .exe (older hosts leave it out). */
  games?: CoopGame[];
  /** Why Nucleus Co-op's handlers and games did not load. */
  catalog_error?: string | null;
  sessions: CoopSession[];
}
/** `GET /api/coop/sessions`: the sessions of `GET /api/coop` alone. */
export type CoopSessions = Pick<CoopStatus, 'sessions'>;
