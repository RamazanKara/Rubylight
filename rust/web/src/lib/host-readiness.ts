import type { Metadata, PowerStatus } from './api';

export type Tone = 'neutral' | 'ok' | 'warn' | 'danger';

/** What a fix button does on a click; `host-fixes.svelte.ts` makes the call. */
export type Fix =
  /** Applies the adapter changes `GET /api/power` listed, as the Power panel's Apply does. */
  | { kind: 'wake'; label: string; changes: string[]; previous: number | null }
  /** Saves settings that apply at once. */
  | { kind: 'settings'; label: string; values: Record<string, string | boolean>; done: string };

export interface Check {
  /** Stable: the key of its row and of its fix while that runs. */
  id: string;
  title: string;
  tone: Tone;
  state: string;
  /** Home's title while this is the problem it leads with. */
  headline: string;
  /** One line: what the PC found. */
  text: string;
  /** What to do about a problem no button can solve. */
  help?: string;
  /** The host's own text, shown as is behind Details. */
  detail?: string;
  /** Where its settings are, and that link's label. */
  href?: string;
  settings?: string;
  /** Nothing streams until it's solved, so it comes before pairing a device. */
  blocking?: boolean;
  fix?: Fix;
  /** A host warning's code: dismissed rather than fixed. */
  warning?: string;
}

/** A fix that saves settings which apply at once. */
const setting = (label: string, values: Record<string, string | boolean>, done: string): Fix => ({ kind: 'settings', label, values, done });

const list = new Intl.ListFormat('en', { style: 'long', type: 'conjunction' });
const plural = (count: number, one: string, many: string) => `${count} ${count === 1 ? one : many}`;

const CAPTURE: Record<string, string> = {
  'idd-push': 'Direct driver push',
  wgc: 'Windows Graphics Capture',
  dxgi: 'Desktop Duplication',
  ddx: 'Desktop Duplication',
};

function encoder(meta: Metadata): Check {
  const base = { id: 'encoder', title: 'Video encoder', href: '/settings/picture', settings: 'Encoder settings' };
  const status = meta.encoder_status;
  const warning = meta.warnings?.find((warning) => warning.code === 'video_encoder');
  if (warning) {
    return {
      ...base, tone: 'danger', state: 'Retrying', blocking: true,
      headline: 'The video encoder can’t start',
      text: 'The video encoder could not start. Rubylight keeps retrying.',
      help: 'Check the selected encoder and graphics adapter in Encoder settings. If they are correct, update or reinstall the graphics driver.',
      detail: warning.message,
    };
  }
  if (status.state === 'checking') {
    return { ...base, tone: 'neutral', state: 'Checking', headline: 'Checking the encoder', text: 'Testing which codecs the graphics card can encode.' };
  }
  const codecs = [
    status.h264 && 'H.264',
    status.hevc && 'HEVC',
    status.av1 && 'AV1',
    status.pyrowave && 'PyroWave',
  ].filter((codec): codec is string => Boolean(codec));
  if (status.state === 'failed' || !codecs.length) {
    return {
      ...base,
      tone: 'danger',
      state: 'Unavailable',
      blocking: true,
      headline: 'No video encoder',
      text: 'No video encoder available. Rubylight keeps retrying.',
      help: 'Check the selected encoder and graphics adapter in Encoder settings, then check the graphics driver.',
    };
  }
  const full = yuv444(status.yuv444);
  if (!status.h264) {
    return {
      ...base,
      tone: 'warn',
      state: 'Limited',
      headline: 'Some devices can’t stream',
      text: `Encodes ${list.format(codecs)}.${full} Devices that only decode H.264 can't stream.`,
    };
  }
  return { ...base, tone: 'ok', state: 'Ready', headline: 'Ready', text: `Encodes ${list.format(codecs)}.${full}` };
}

/** The native YUV 4:4:4 modes as a sentence, empty when there are none. */
function yuv444(modes: string[] | undefined): string {
  const names = (modes ?? []).map((mode) => mode.replace(/ 4:4:4$/, ''));
  return names.length ? ` YUV 4:4:4 in ${list.format(names)}.` : '';
}

/** Monitors Windows lists as on. */
const monitors = (meta: Metadata) => meta.capture_status.displays?.length ?? 0;

function virtualDisplay(meta: Metadata): Check {
  const base = { id: 'virtual-display', title: 'Virtual display', href: '/settings/picture', settings: 'Display settings' };
  const driver = meta.virtual_display;
  const on = meta.capture_status.virtual_display_configured;
  const detail = driver.reason || undefined;
  if (driver.ready) {
    return on
      ? { ...base, tone: 'ok', state: 'Ready', headline: 'Ready', text: 'Streams use a virtual display.' }
      : { ...base, tone: 'neutral', state: 'Off', headline: 'Off', text: 'The driver is ready. Streams use a physical display.' };
  }
  if (driver.capable) {
    return {
      ...base, tone: 'warn', state: 'Not ready', headline: 'The virtual display isn’t ready',
      text: "The driver is installed but isn't ready yet.", detail,
    };
  }
  if (on) {
    // With a monitor on, one click streams it instead; without one nothing streams.
    const monitor = monitors(meta) > 0;
    return {
      ...base,
      tone: 'danger',
      state: 'Unavailable',
      blocking: !monitor,
      headline: 'The virtual display can’t open',
      text: "It's turned on, but the driver can't be opened.",
      ...(monitor
        ? { fix: setting('Stream a monitor instead', { virtual_display_mode: 'disabled' }, 'Streams use a monitor.') }
        : { help: 'Check that the Rubylight service is running, or turn off the virtual display to use a monitor.' }),
      detail,
    };
  }
  return { ...base, tone: 'neutral', state: 'Off', headline: 'Off', text: 'Not available. Streams use a physical display.', detail };
}

function audio(meta: Metadata): Check {
  const base = { id: 'audio', title: 'Audio', href: '/settings/sound', settings: 'Audio settings' };
  if (!meta.audio_enabled) {
    return { ...base, tone: 'neutral', state: 'Off', headline: 'Off', text: 'Audio streaming is turned off. Streams have no sound.' };
  }
  if (meta.audio_error) {
    return { ...base, tone: 'danger', state: 'Error', headline: 'No sound on streams', text: "Audio outputs can't be read.", detail: meta.audio_error };
  }
  const sinks = meta.audio_sinks ?? [];
  if (!sinks.length) {
    return { ...base, tone: 'warn', state: 'No output', headline: 'No sound on streams', text: 'No audio output was found on this PC.' };
  }
  const fallback = sinks.find((sink) => sink.default);
  const name = fallback ? fallback.name || fallback.description : '';
  return {
    ...base,
    tone: 'ok',
    state: 'Ready',
    headline: 'Ready',
    text: `${plural(sinks.length, 'output', 'outputs')} found.${name ? ` Default is ${name}.` : ''}`,
  };
}

function capture(meta: Metadata): Check {
  const base = { id: 'capture', title: 'Screen capture', href: '/settings/picture', settings: 'Video settings' };
  const status = meta.capture_status;
  const backend = status.configured_backend || 'auto';
  const method = backend === 'auto'
    ? 'Uses direct driver push on Rubylight virtual displays and Windows Graphics Capture on other displays.'
    : backend === 'idd-push'
      ? `Prefers ${CAPTURE[backend]} on Rubylight virtual displays, with Windows capture as fallback.`
      : `Uses ${CAPTURE[backend] ?? 'the configured capture method'}.`;
  if (status.error) {
    return {
      ...base, tone: 'danger', state: 'Error', blocking: true, headline: 'Displays can’t be read',
      text: "Windows didn't list this PC's displays.", detail: status.error,
    };
  }
  const displays = monitors(meta);
  if (displays) {
    return { ...base, tone: 'ok', state: 'Ready', headline: 'Ready', text: `${method} ${plural(displays, 'display', 'displays')} found.` };
  }
  if (status.virtual_display_configured && meta.virtual_display.ready) {
    return { ...base, tone: 'ok', state: 'Ready', headline: 'Ready', text: `${method} No monitor is on; streams use the virtual display.` };
  }
  // The driver is ready but streams are set to use a monitor: one click streams the virtual display.
  const virtual = meta.virtual_display.ready && !status.virtual_display_configured;
  return {
    ...base,
    tone: 'warn',
    state: 'No display',
    blocking: true,
    headline: 'No display to stream',
    text: 'No active display was found.',
    ...(virtual
      ? { fix: setting('Use a virtual display', { virtual_display_mode: 'per_client' }, 'Streams use a virtual display.') }
      : { help: 'Turn on a monitor or set up a virtual display.' }),
  };
}

/**
 * Whether devices can wake this PC, from the adapter `/serverinfo` reports. Only a state
 * one click changes shows here; the rest is the Power panel's (Settings).
 */
export function wakeCheck(power: PowerStatus): Check | null {
  const target = power.wake.targets.find((target) => target.mac === power.wake.primary);
  if (!target) return null;
  const base = { id: 'wake', title: 'Wake from a device', href: '/settings', settings: 'Wake settings' };
  if (target.state === 'ready') {
    return { ...base, tone: 'ok', state: 'Ready', headline: 'Ready', text: 'Devices can wake this PC from sleep.' };
  }
  const changes = power.fix.changes ?? [];
  if (target.state !== 'needs_setup' || !changes.length || power.fix.running) return null;
  return {
    ...base,
    tone: 'warn',
    state: 'Off',
    headline: 'Devices can’t wake this PC',
    text: 'A wake setting of its network adapter is off. Turning it on restarts the adapter for a few seconds.',
    fix: { kind: 'wake', label: 'Turn on', changes, previous: power.fix.last?.at ?? null },
  };
}

interface HostWarning {
  code: string;
  title: string;
  /** The host's own text. */
  message: string;
}

const WARNING_TITLES: Record<string, string> = {
  settings_retired: 'Settings replaced',
  nvidia_limiter_retired: 'NVIDIA frame limiter setting removed',
  codec_support: 'Codecs missing',
  firewall_blocked: 'Windows Firewall blocks Rubylight',
  webrtc_server: 'Browser streaming can’t start',
};

/** Where a warning's setting is. */
const WARNING_LINKS: Record<string, { href: string; settings: string }> = {
  webrtc_server: { href: '/settings/network?setting=webrtc_port', settings: 'WebRTC port' },
};

/** The host's warnings beyond the checks: each shown until it's dismissed. */
export function hostWarnings(metadata: Metadata): HostWarning[] {
  return (metadata.warnings ?? [])
    // The encoder check shows this one, and it clears itself on recovery.
    .filter((warning) => warning.code !== 'video_encoder')
    .map((warning) => ({ ...warning, title: WARNING_TITLES[warning.code] ?? 'Rubylight warning' }));
}

/** A dismissed warning stays hidden until its text changes: the key it's hidden by. */
export const warningKey = (warning: { code: string; message: string }) => `${warning.code}\n${warning.message}`;

/** The checks; with the wake status, also whether devices can wake this PC. */
export function hostChecks(metadata: Metadata, power?: PowerStatus | null): Check[] {
  const wake = power ? wakeCheck(power) : null;
  return [encoder(metadata), virtualDisplay(metadata), audio(metadata), capture(metadata), ...(wake ? [wake] : [])];
}

export const isProblem = (check: Check) => check.tone === 'warn' || check.tone === 'danger';

/** Ranks a problem: what stops every stream, then what one click fixes, then the rest, host warnings last. */
const rank = (check: Check) => (check.blocking ? 0 : check.fix ? 1 : check.warning ? 3 : 2);

/** What needs a look, the one Home leads with first; `dismissed` holds `warningKey`s. */
export function problems(metadata: Metadata, power?: PowerStatus | null, dismissed: string[] = []): Check[] {
  const warnings = hostWarnings(metadata)
    .filter((warning) => !dismissed.includes(warningKey(warning)))
    .map((warning): Check => ({
      id: `warning:${warning.code}`,
      title: warning.title,
      tone: 'warn',
      state: 'Warning',
      headline: warning.title,
      text: warning.message,
      warning: warning.code,
      ...WARNING_LINKS[warning.code],
    }));
  // Array sort is stable: equal ranks keep the checks' order.
  return [...hostChecks(metadata, power).filter(isProblem), ...warnings].sort((a, b) => rank(a) - rank(b));
}

/** Whether Home lists the checks on its own: a check that warns, or a host warning not yet dismissed. */
export function needsAttention(metadata: Metadata, power?: PowerStatus | null): boolean {
  return problems(metadata, power).length > 0;
}

/** What Home's top leads with. */
export type Lead = 'unavailable' | 'checking' | 'live' | 'blocked' | 'pair' | 'problem' | 'ready';

/**
 * A stream in progress first; then what stops every stream; then pairing the first
 * device, which a problem that only limits streams never pushes down; then that problem.
 */
export function homeLead(state: {
  /** The PC's state could not be read. */
  unavailable: boolean;
  /** Details, streams and the running app have loaded. */
  loaded: boolean;
  live: boolean;
  paired: boolean;
  /** The encoder check still runs. */
  probing: boolean;
  /** The first of `problems`. */
  top: Check | undefined;
}): Lead {
  if (state.unavailable) return 'unavailable';
  if (!state.loaded) return 'checking';
  if (state.live) return 'live';
  if (state.top?.blocking) return 'blocked';
  if (!state.paired) return 'pair';
  if (state.top) return 'problem';
  return state.probing ? 'checking' : 'ready';
}
