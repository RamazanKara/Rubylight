// Wake-on-LAN readiness and sleep after idle (`GET /api/power`) in words.
import type { PowerStatus, WakeCheck, WakeTarget } from './api';
import { duration } from './format';

export type Tone = 'neutral' | 'ok' | 'warn' | 'danger';
type Pill = 'ready' | 'warning' | 'idle' | 'error';

/** The adapter settings as Windows' Device Manager names them. */
const CHECKS: Record<string, string> = {
  wake_enabled: 'Allow this device to wake the computer',
  magic_packet: 'Wake on magic packet',
  magic_packet_only: 'Only allow a magic packet to wake the computer',
  wake_on_pattern: 'Wake on pattern match',
  modern_standby_magic_packet: 'Wake on magic packet from Modern Standby',
  shutdown_wake: 'Wake from shutdown',
};

/** A check's name; ids from a newer host are shown as they are. */
export function checkName(id: string): string {
  return CHECKS[id] ?? id;
}

const CHECK_STATES: Record<string, { word: string; tone: Tone }> = {
  ok: { word: 'OK', tone: 'ok' },
  warning: { word: 'Worth changing', tone: 'warn' },
  error: { word: 'Needs fixing', tone: 'danger' },
};

export function checkState(check: WakeCheck): { word: string; tone: Tone } {
  return CHECK_STATES[check.state] ?? { word: 'Unreadable', tone: 'neutral' };
}

const TARGET_STATES: Record<string, { status: Pill; label: string }> = {
  ready: { status: 'ready', label: 'Ready' },
  needs_setup: { status: 'warning', label: 'Needs setup' },
  experimental: { status: 'idle', label: 'Depends on Wi-Fi' },
};

export function targetState(target: WakeTarget): { status: Pill; label: string } {
  return TARGET_STATES[target.state] ?? { status: 'warning', label: 'Unknown' };
}

export function adapterKind(target: WakeTarget): string {
  return target.kind === 'ethernet' ? 'Wired' : target.kind === 'wifi' ? 'Wi-Fi' : 'Network';
}

export interface Readiness {
  status: Pill;
  label: string;
  tone: Tone;
  text: string;
}

/** Whether devices can wake this PC, judged by the adapter `/serverinfo` reports (on this PC: the default route's). */
export function readiness(power: PowerStatus): Readiness {
  const { targets, primary } = power.wake;
  if (!targets.length) {
    return { status: 'error', label: 'Unavailable', tone: 'danger', text: 'No physical network adapter was found. Devices can wake this PC only through a wired or Wi-Fi adapter.' };
  }
  const target = targets.find((target) => target.mac === primary);
  if (!target) {
    // Opened on this PC without a default route: the host can't tell which adapter devices reach.
    if (targets.some((target) => target.connected)) {
      return { status: 'idle', label: 'Check from another device', tone: 'neutral', text: 'This PC has no default route, so it cannot tell which adapter devices reach. Open this page from another device to see which adapter it uses.' };
    }
    return { status: 'warning', label: 'Not connected', tone: 'warn', text: 'No physical adapter is connected, so devices get no address to wake this PC.' };
  }
  const name = `the ${adapterKind(target).toLowerCase()} adapter ${target.mac}`;
  switch (target.state) {
    case 'ready':
      return { status: 'ready', label: 'Ready', tone: 'ok', text: `Devices can wake this PC through ${name}.` };
    case 'needs_setup':
      return { status: 'warning', label: 'Needs setup', tone: 'warn', text: `A wake setting of ${name} is off. Apply the changes below to turn it on.` };
    case 'experimental':
      return { status: 'idle', label: 'Depends on Wi-Fi', tone: 'neutral', text: `Devices wake this PC through ${name}. Waking through Wi-Fi depends on the adapter and the access point.` };
    default:
      return { status: 'warning', label: 'Unknown', tone: 'warn', text: `The wake settings of ${name} could not be read. Check again, or look at the adapter in Device Manager.` };
  }
}

/** What the PC's power settings mean for waking it and for sleep after idle. */
export function systemNotes(power: PowerStatus): string[] {
  const system = power.wake.system;
  const notes: string[] = [];
  if (system.modern_standby) notes.push('This PC uses Modern Standby, so Rubylight cannot put it to sleep.');
  else if (system.sleep === 'none') notes.push('This PC has no sleep state (S3), so Rubylight cannot put it to sleep.');
  if (system.fast_startup === 'active') notes.push('Fast Startup is on: Shut down hibernates the PC, and an adapter may not wake it from that.');
  if (system.shutdown_wake === 'firmware_unknown') notes.push('Waking from Shut down also depends on firmware (BIOS or UEFI) settings that Windows cannot read.');
  return notes;
}

export function sleepState(power: PowerStatus): string {
  const system = power.wake.system;
  if (system.sleep === 's3') return 'Available (S3)';
  if (system.sleep === 'none') return system.modern_standby ? 'Modern Standby only' : 'Not available';
  return 'Unknown';
}

const FAST_STARTUP: Record<string, string> = {
  active: 'On',
  inactive: 'On, but hibernation is off, so it has no effect',
  off: 'Off',
};

export function fastStartup(power: PowerStatus): string {
  return FAST_STARTUP[power.wake.system.fast_startup] ?? 'Unknown';
}

const sentence = (text: string) => text.charAt(0).toUpperCase() + text.slice(1);

/** The wait the countdown follows, as the host names it. */
const WAITS: Record<string, string> = {
  'waiting for quiet': 'counted from the last stream, wake or sleep',
  'waiting for local input': "counted from the last use of this PC's keyboard or mouse",
  'waiting for console activity': 'counted from the last use of this console',
};

/** Sleep after idle: its state and countdown. */
export function idleSleep(power: PowerStatus): { title: string; tone: Tone; text?: string } {
  const idle = power.idle_sleep;
  switch (idle.state) {
    case 'off':
      return { title: 'Off', tone: 'neutral' };
    case 'armed': {
      const title = `After ${duration(idle.minutes * 60)} idle`;
      const left = idle.remaining_seconds;
      if (left === null) return { title, tone: 'ok' };
      if (left === 0) return { title, tone: 'ok', text: 'The PC goes to sleep now.' };
      const wait = idle.reason ? ` (${WAITS[idle.reason] ?? idle.reason})` : '';
      return { title, tone: 'ok', text: `Sleeps in ${duration(left)} if nothing changes${wait}.` };
    }
    case 'blocked':
      // Minutes are 0 when the setting itself could not be read.
      return idle.minutes
        ? { title: `Paused (after ${duration(idle.minutes * 60)} idle)`, tone: 'neutral', text: `${sentence(idle.reason ?? 'something keeps the PC awake')}.` }
        // The host's error names the config key: keep its spelling.
        : { title: 'Not working', tone: 'danger', text: `${idle.reason ?? 'The setting could not be read'}.` };
    case 'unsupported':
      return { title: 'Unavailable on this PC', tone: 'warn', text: `${sentence(idle.reason ?? 'this PC cannot sleep')}.` };
    default:
      return { title: 'Unknown', tone: 'warn', text: 'The host reported a state this console does not know. Update Rubylight.' };
  }
}

const isObject = (value: unknown): value is Record<string, unknown> =>
  typeof value === 'object' && value !== null && !Array.isArray(value);

/** A reply without the fields this page shows is an error, not an empty page. */
export function checkPowerStatus(value: unknown): PowerStatus {
  const wake = isObject(value) ? value.wake : undefined;
  const idle = isObject(value) ? value.idle_sleep : undefined;
  const fix = isObject(value) ? value.fix : undefined;
  if (
    !isObject(wake) || !Array.isArray(wake.targets) || !isObject(wake.system) ||
    !isObject(idle) || typeof idle.state !== 'string' ||
    !isObject(fix) || typeof fix.running !== 'boolean'
  ) {
    throw new Error('The host sent an incomplete power status.');
  }
  return value as unknown as PowerStatus;
}
