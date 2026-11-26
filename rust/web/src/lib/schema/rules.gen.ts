// Generated from rust/core/config/settings.toml by tools/generated_docs.py; do not edit by hand.
import type { ConfigValue } from '../api';
import { NATIVE_ENCODERS, VIRTUAL_DISPLAY_OS, offersEnhancement, type Aliases, type Option } from '../settings-types';
import { toList } from '../../components/settings/values';

export type Values = Record<string, ConfigValue>;

/** A boolean read as the host reads it; anything unrecognised keeps the default. */
export function on(values: Values, key: string, fallback: boolean): boolean {
  const value = values[key];
  if (typeof value === 'boolean') return value;
  const word = String(value ?? '').trim().replace(/^"(.*)"$/, '$1').toLowerCase();
  if (['true', 'yes', '1', 'enable', 'enabled', 'on'].includes(word)) return true;
  if (['false', 'no', '0', 'disable', 'disabled', 'off'].includes(word)) return false;
  return fallback;
}

/** A text setting read as basics.ts reads it: blank means the default. */
export function text(values: Values, key: string, fallback: string): string {
  const value = values[key];
  const raw = value === null || value === undefined ? '' : String(value);
  return raw.trim() === '' ? fallback : raw;
}

/** A text setting trimmed as video.ts/display.ts read it: unset or blank means the default. */
export function read(values: Values, key: string, fallback: string): string {
  const value = values[key];
  if (value === undefined || value === null) return fallback;
  const trimmed = String(value).trim();
  return trimmed === '' ? fallback : trimmed;
}

export const streamsAudio = (values: Values): boolean => on(values, 'stream_audio', true);
export const systemAudio = (values: Values): boolean => streamsAudio(values) && text(values, 'audio_capture_mode', 'system') === 'system';
export const keyboardOn = (values: Values): boolean => on(values, 'keyboard', true);
export const mouseOn = (values: Values): boolean => on(values, 'mouse', true);
export const controllerOn = (values: Values): boolean => on(values, 'controller', true);
export const couchMenuOn = (values: Values): boolean => on(values, 'couch_menu', true);
export const rumbleOn = (values: Values): boolean => controllerOn(values) && on(values, 'forward_rumble', true);
export const audioHapticsOn = (values: Values): boolean => rumbleOn(values) && on(values, 'audio_haptics', false);

export const streamsWindow = (values: Values): boolean =>
  ['window', 'app', 'true', 'on', 'yes', '1', 'enabled', 'window_focus', 'focus'].includes(read(values, 'capture_window', 'display').trim().toLowerCase());
export const gridPacing = (values: Values): boolean => ['grid', 'fixed'].includes(read(values, 'frame_pacing', 'arrival').toLowerCase());

function encoder(values: Values): string {
  const name = read(values, 'encoder', 'auto');
  switch (name) {
    case 'amdvce': case 'amdvce_experimental': case 'amdvce_ffmpeg': case 'amdvce_legacy': return 'amf';
    case 'nvenc_experimental': case 'nvenc_legacy': return 'nvenc';
    case 'amf': case 'nvenc': return name;
    default: return 'auto';
  }
}

function automatic(values: Values, backend: string, unknown: boolean): boolean {
  if (encoder(values) !== 'auto') return false;
  const native = values[NATIVE_ENCODERS];
  return Array.isArray(native) && native.length > 0 ? native.includes(backend) : unknown;
}

export const amf = (values: Values): boolean => encoder(values) === 'amf' || automatic(values, 'amf', true);
export const nvenc = (values: Values): boolean => encoder(values) === 'nvenc' || automatic(values, 'nvenc', false);
export const rtxVideo = (values: Values): boolean => offersEnhancement(values, 'rtx_video');
export const rtxHdr = (values: Values): boolean => offersEnhancement(values, 'rtx_hdr');
export const virtualDisplayOs = (values: Values): boolean => values[VIRTUAL_DISPLAY_OS] !== false;
export const VIRTUAL_DISPLAY_OS_GATE = (): string => 'Needs Windows 11 23H2 or later';

export function rtss(values: Values): boolean {
  const name = read(values, 'frame_limiter_provider', 'auto').toLowerCase().replace(/[^a-z0-9]/g, '');
  return !['none', 'disabled'].includes(name);
}

export function pyrowaveCustomFec(values: Values): boolean {
  return on(values, 'pyrowave', true) && values.pyrowave_critical_fec_percentage != null && Number(values.pyrowave_critical_fec_percentage) !== Number(values.fec_percentage ?? 20);
}

export function keyframeLossPacing(values: Values): boolean {
  const pacing = read(values, 'packet_pacing', 'auto');
  return pacing !== 'always' && !(pacing !== 'off' && Number(pacing) > 0);
}

export function amfUsageCaution(usage: string): string | undefined {
  switch (usage) {
    case 'lowlatency_high_quality':
      return 'Slower than Ultra low latency: about 0.4 ms more per HEVC frame and 0.9 ms per AV1 frame at 1440p120 (RX 7900 XT).';
    case 'auto':
      return "Adds delay: the driver's default is its transcoding profile, which leaves the low-latency modes off and queues frames at 4K.";
    case 'transcoding': case 'lowlatency': case 'high_quality':
      return 'Adds delay: this profile queues frames at 4K. Ultra low latency is the fastest.';
    default:
      return undefined;
  }
}

export function rtssReflexCaution(values: Values): string | undefined {
  const encoders = values[NATIVE_ENCODERS];
  const reflex = read(values, 'rtss_frame_limit_type', 'async').toLowerCase().replace(/[^a-z0-9]/g, '') === 'nvidiareflex';
  return reflex && Array.isArray(encoders) && encoders.length > 0 && !encoders.includes('nvenc')
    ? 'This PC streams without an NVIDIA GPU, so NVIDIA Reflex limits with front edge sync instead.'
    : undefined;
}

export const steam = (values: Values): boolean => on(values, 'steam_enabled', true);
export const folders = (values: Values): boolean => toList(values.library_local_folders).some((folder) => folder.trim() !== '');
export const playnite = (values: Values): boolean => on(values, 'playnite_enabled', true);
export const sync = (values: Values): boolean => on(values, 'library_auto_sync', true);
export const syncAll = (values: Values): boolean => on(values, 'library_sync_all_installed', true);
export const recent = (values: Values): boolean => !syncAll(values);
export const playniteSync = (values: Values): boolean => playnite(values) && sync(values);
export const playniteRecent = (values: Values): boolean => playniteSync(values) && !syncAll(values);

export const locales: Option[] = [
  { value: 'en', label: 'English' },
  { value: 'en_GB', label: 'English (UK)' },
  { value: 'en_US', label: 'English (US)' },
  { value: 'bg', label: 'Български' },
  { value: 'cs', label: 'Čeština' },
  { value: 'de', label: 'Deutsch' },
  { value: 'es', label: 'Español' },
  { value: 'fr', label: 'Français' },
  { value: 'hu', label: 'Magyar' },
  { value: 'it', label: 'Italiano' },
  { value: 'ja', label: '日本語' },
  { value: 'ko', label: '한국어' },
  { value: 'pl', label: 'Polski' },
  { value: 'pt', label: 'Português' },
  { value: 'pt_BR', label: 'Português (Brasil)' },
  { value: 'ru', label: 'Русский' },
  { value: 'sv', label: 'Svenska' },
  { value: 'tr', label: 'Türkçe' },
  { value: 'uk', label: 'Українська' },
  { value: 'vi', label: 'Tiếng Việt' },
  { value: 'zh', label: '中文（简体）' },
  { value: 'zh_TW', label: '中文（繁體）' },
];
export const encryption: Option[] = [
  { value: '0', label: 'Off' },
  { value: '1', label: 'When the device supports it' },
  { value: '2', label: 'Required' },
];
export const gripOptions: Option[] = [
  { value: 'none', label: 'Nothing' },
  { value: 'a', label: 'A (Cross)' },
  { value: 'b', label: 'B (Circle)' },
  { value: 'x', label: 'X (Square)' },
  { value: 'y', label: 'Y (Triangle)' },
  { value: 'lb', label: 'Left bumper (L1)' },
  { value: 'rb', label: 'Right bumper (R1)' },
  { value: 'lt', label: 'Left trigger (L2), fully' },
  { value: 'rt', label: 'Right trigger (R2), fully' },
  { value: 'l3', label: 'Left stick click (L3)' },
  { value: 'r3', label: 'Right stick click (R3)' },
  { value: 'back', label: 'View / Back (Create, Share)' },
  { value: 'start', label: 'Menu / Start (Options)' },
  { value: 'guide', label: 'Guide (PS)' },
  { value: 'dpad_up', label: 'D-pad up' },
  { value: 'dpad_down', label: 'D-pad down' },
  { value: 'dpad_left', label: 'D-pad left' },
  { value: 'dpad_right', label: 'D-pad right' },
  { value: 'touchpad', label: 'Touchpad click' },
  { value: 'misc', label: 'Share (Xbox) / Mute (DualSense)' },
];
export const tristate: Option[] = [
  { value: 'auto', label: 'Driver default' },
  { value: 'enabled', label: 'On' },
  { value: 'disabled', label: 'Off' },
];
export const adjust: Option[] = [
  { value: 'disabled', label: 'Leave as it is' },
  { value: 'auto', label: 'Match the device' },
  { value: 'manual', label: 'Choose below' },
];
export const scales: Option[] = [
  { value: '0', label: 'Keep' },
  { value: '-1', label: 'Recommended' },
  { value: '100', label: '100%' },
  { value: '125', label: '125%' },
  { value: '150', label: '150%' },
  { value: '175', label: '175%' },
  { value: '200', label: '200%' },
  { value: '225', label: '225%' },
  { value: '250', label: '250%' },
  { value: '300', label: '300%' },
  { value: '350', label: '350%' },
  { value: '400', label: '400%' },
  { value: '450', label: '450%' },
  { value: '500', label: '500%' },
];
export const rotations: Option[] = [
  { value: '0', label: '0°' },
  { value: '90', label: '90°' },
  { value: '180', label: '180°' },
  { value: '270', label: '270°' },
];

export const switchWords: Aliases = { match: 'case', spellings: { '0': 'disabled', '1': 'enabled', true: 'enabled', yes: 'enabled', enable: 'enabled', on: 'enabled', false: 'disabled', no: 'disabled', disable: 'disabled', off: 'disabled' } };
export const coderWords: Aliases = { spellings: { ac: 'cabac', vlc: 'cavlc' } };
export const splitWords: Aliases = { match: 'case', spellings: { '0': 'disabled', '1': 'enabled', true: 'enabled', yes: 'enabled', enable: 'enabled', on: 'enabled', false: 'disabled', no: 'disabled', disable: 'disabled', off: 'disabled' } };
