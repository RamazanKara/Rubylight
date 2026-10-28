// A game's Radeon profile: the app's own `radeon-profile` and, for Custom, the Radeon
// settings it lists in `radeon-settings`. The host checks both when the app is saved
// (rust/core/src/radeon_profile.rs); the checks here only spare a round trip.
import type { RadeonPanel, RadeonSetting } from '../../lib/api';
import table from '../../lib/radeon_profiles.json';

export const PROFILE_KEY = table.profile_key;
export const SETTINGS_KEY = table.settings_key;
/** The most settings a Custom profile lists. */
export const MAX_SETTINGS = 24;

export interface ProfileInfo {
  id: string;
  label: string;
  description: string;
}
export const profiles: ProfileInfo[] = table.profiles.map(({ id, label, description }) => ({ id, label, description }));

export type SettingValue = boolean | number | string;
export interface SettingRow {
  /** Keeps each row's inputs in place when rows move. */
  key: number;
  id: string;
  /** What is typed: true, false, a whole number or text. */
  value: string;
}

let nextKey = 1;
export const settingRow = (id = '', value = ''): SettingRow => ({ key: nextKey++, id, value });

/** The profile an app names, in the spelling the host reads. */
export function profileOf(value: unknown): string {
  return typeof value === 'string' ? value.trim().toLowerCase() : '';
}

/** `true` and `false` are switches, digits are a whole number, anything else is text. */
export function parseValue(text: string): SettingValue {
  const trimmed = text.trim();
  if (trimmed === 'true') return true;
  if (trimmed === 'false') return false;
  if (/^-?\d+$/.test(trimmed) && Number.isSafeInteger(Number(trimmed))) return Number(trimmed);
  return trimmed;
}

export function formatValue(value: unknown): string {
  return typeof value === 'boolean' || typeof value === 'number' || typeof value === 'string' ? String(value) : '';
}

/** The rows for a stored `radeon-settings`; anything that is not an object gives none. */
export function rowsOf(settings: unknown): SettingRow[] {
  if (!settings || typeof settings !== 'object' || Array.isArray(settings)) return [];
  return Object.entries(settings as Record<string, unknown>).map(([id, value]) => settingRow(id, formatValue(value)));
}

const NUMBER_LIKE = /^[-+]?(\d+\.?\d*|\.\d+)(e[-+]?\d+)?$/i;
const ID =/^[a-z0-9_]+(\.[a-z0-9_]+)+$/;

/** What is wrong with a row, or ''. A blank row is dropped when saving and is not an error. */
export function rowError(row: SettingRow, rows: SettingRow[]): string {
  const id = row.id.trim();
  if (!id && !row.value.trim()) return '';
  if (!id) return 'Name the setting.';
  if (id.length > 64 || !ID.test(id)) return 'Use a Radeon setting id such as frame_rate_target.fps.';
  if (rows.some((other) => other !== row && other.id.trim() === id)) return 'Listed twice.';
  const value = row.value.trim();
  if (!value) return 'Give the setting a value.';
  if (value.length > 64) return 'The value is too long.';
  // Settings are switches, whole numbers or named choices: a number with a fraction, an
  // exponent or too many digits is none of them, and would be saved as text the driver refuses.
  if (NUMBER_LIKE.test(value) && typeof parseValue(value) !== 'number') return 'Use a whole number.';
  return '';
}

/** The object to store: blank rows are dropped. */
export function settingsOf(rows: SettingRow[]): Record<string, SettingValue> {
  const settings: Record<string, SettingValue> = {};
  for (const row of rows) {
    if (row.id.trim()) settings[row.id.trim()] = parseValue(row.value);
  }
  return settings;
}

/** What stops this profile choice from being saved, or ''. */
export function profileError(profile: unknown, rows: SettingRow[]): string {
  const name = profileOf(profile);
  if (name !== 'custom') return '';
  const listed = rows.filter((row) => row.id.trim() || row.value.trim());
  if (!listed.length) return 'List at least one setting for a custom profile.';
  if (listed.length > MAX_SETTINGS) return `List at most ${MAX_SETTINGS} settings.`;
  return listed.map((row) => rowError(row, rows)).find(Boolean) ?? '';
}

// ---------- controls ----------
// The game page shows a preset picker and plain controls; both only ever produce
// `radeon-profile` and, for Custom, the full `radeon-settings` list. What the controls
// cannot show stays as raw rows, so nothing typed is lost.

/** The value that stands for the stream's frame rate. */
export const STREAM_FPS = table.stream_fps;

type Settings = Record<string, SettingValue>;

/** What a preset writes, or null for Off, Custom and profiles this console does not know. */
export function presetSettings(profile: unknown): Settings | null {
  const name = profileOf(profile);
  if (name === 'custom') return null;
  const found = table.profiles.find((profile) => profile.id === name);
  if (!found) return null;
  const settings: Settings = {};
  for (const { id, value } of found.settings as { id: string; value: SettingValue }[]) settings[id] = value;
  return settings;
}

/** The settings a profile choice writes: a preset's own, Custom's rows, none for Off. */
export function effectiveSettings(profile: unknown, rows: SettingRow[]): Settings {
  return profileOf(profile) === 'custom' ? settingsOf(rows) : (presetSettings(profile) ?? {});
}

function sameSettings(a: Settings, b: Settings): boolean {
  const keys = Object.keys(a);
  return keys.length === Object.keys(b).length && keys.every((key) => key in b && a[key] === b[key]);
}

/** The profile a list of settings is: Off when empty, a preset when equal to one, else Custom. */
export function profileFor(settings: Settings): string {
  if (!Object.keys(settings).length) return '';
  const preset = table.profiles.find(
    ({ id }) => id !== 'custom' && sameSettings(settings, presetSettings(id) ?? {}),
  );
  return preset?.id ?? 'custom';
}

/** The settings the controls may write: supported and writable on this PC's GPU, by id. */
export type Support = Map<string, RadeonSetting>;

/** The first GPU's configurable settings; empty without a Radeon GPU or a reading. */
export function supportOf(panel: RadeonPanel | null | undefined): Support {
  const support: Support = new Map();
  for (const setting of panel?.gpus?.[0]?.settings ?? []) {
    if (setting.state === 'configurable') support.set(setting.id, setting);
  }
  return support;
}

export type FrameCap = 'off' | 'stream' | 'fps';
export type AntiAliasing = 'game' | 'enhance' | 'override';
export interface Controls {
  antiLag: boolean;
  frameCap: FrameCap;
  /** null leaves the driver's own number. */
  fps: number | null;
  chill: boolean;
  chillMin: number | null;
  chillMax: number | null;
  sharpening: boolean;
  sharpness: number | null;
  anisotropic: boolean;
  antiAliasing: AntiAliasing;
  aaLevel: string | null;
  aaMethod: string | null;
  boost: boolean;
  boostResolution: number | null;
}
export const NO_CONTROLS: Controls = {
  antiLag: false,
  frameCap: 'off',
  fps: null,
  chill: false,
  chillMin: null,
  chillMax: null,
  sharpening: false,
  sharpness: null,
  anisotropic: false,
  antiAliasing: 'game',
  aaLevel: null,
  aaMethod: null,
  boost: false,
  boostResolution: null,
};

/** Each control and the setting the GPU must support for it to be shown. */
const FEATURES = {
  antiLag: ['anti_lag.enabled'],
  frameCap: ['frame_rate_target.enabled', 'frame_rate_target.fps'],
  chill: ['chill.enabled'],
  sharpening: ['image_sharpening.enabled'],
  anisotropic: ['anisotropic_filtering.enabled'],
  antiAliasing: ['anti_aliasing.mode'],
  boost: ['boost.enabled'],
} as const;
export type Feature = keyof typeof FEATURES;

/** The controls this GPU supports; none without support data. */
export function offered(support: Support): Set<Feature> {
  return new Set(
    (Object.keys(FEATURES) as Feature[]).filter((feature) => FEATURES[feature].every((id) => support.has(id))),
  );
}

const whole = (value: unknown): value is number => typeof value === 'number' && Number.isSafeInteger(value);

/** The controls for a list of settings, and the ids they show. The rest are raw rows. */
export function controlsOf(settings: Settings, support: Support): { controls: Controls; used: Set<string> } {
  const controls: Controls = { ...NO_CONTROLS };
  const used = new Set<string>();
  const shown = offered(support);
  // A feature is shown on when its switch is listed as true; its numbers when they are
  // whole numbers the GPU supports. Anything else, such as `.enabled = false`, stays raw.
  const feature = (name: Feature, enabled: string, numbers: string[]): (number | null)[] | null => {
    if (!shown.has(name) || settings[enabled] !== true) return null;
    used.add(enabled);
    return numbers.map((id) => {
      const value = settings[id];
      if (!support.has(id) || !whole(value)) return null;
      used.add(id);
      return value;
    });
  };
  if (feature('antiLag', 'anti_lag.enabled', [])) controls.antiLag = true;
  const fps = settings['frame_rate_target.fps'];
  if (shown.has('frameCap') && settings['frame_rate_target.enabled'] === true && (fps === STREAM_FPS || whole(fps) || fps === undefined)) {
    used.add('frame_rate_target.enabled');
    if (fps !== undefined) used.add('frame_rate_target.fps');
    controls.frameCap = fps === STREAM_FPS ? 'stream' : 'fps';
    controls.fps = whole(fps) ? fps : null;
  }
  const chill = feature('chill', 'chill.enabled', ['chill.min_fps', 'chill.max_fps']);
  if (chill) [controls.chill, controls.chillMin, controls.chillMax] = [true, chill[0] ?? null, chill[1] ?? null];
  const sharpening = feature('sharpening', 'image_sharpening.enabled', ['image_sharpening.sharpness']);
  if (sharpening) [controls.sharpening, controls.sharpness] = [true, sharpening[0] ?? null];
  if (feature('anisotropic', 'anisotropic_filtering.enabled', [])) controls.anisotropic = true;
  const boost = feature('boost', 'boost.enabled', ['boost.min_resolution']);
  if (boost) [controls.boost, controls.boostResolution] = [true, boost[0] ?? null];
  const mode = settings['anti_aliasing.mode'];
  if (shown.has('antiAliasing') && (mode === 'enhance_app_settings' || mode === 'override_app_settings')) {
    used.add('anti_aliasing.mode');
    controls.antiAliasing = mode === 'enhance_app_settings' ? 'enhance' : 'override';
    if (mode === 'override_app_settings') {
      const choice = (id: string): string | null => {
        const value = settings[id];
        const options = support.get(id)?.options ?? [];
        if (typeof value !== 'string' || !support.has(id) || (options.length && !options.some((o) => o.value === value)))
          return null;
        used.add(id);
        return value;
      };
      controls.aaLevel = choice('anti_aliasing.level');
      controls.aaMethod = choice('anti_aliasing.method');
    }
  }
  return { controls, used };
}

/** What the controls write; a setting this GPU does not support is never among them. */
export function settingsOfControls(controls: Controls, support: Support): Settings {
  const settings: Settings = {};
  const shown = offered(support);
  const put = (id: string, value: SettingValue | null) => {
    if (value !== null && support.has(id)) settings[id] = value;
  };
  if (shown.has('antiLag') && controls.antiLag) put('anti_lag.enabled', true);
  if (shown.has('frameCap') && controls.frameCap !== 'off') {
    put('frame_rate_target.enabled', true);
    put('frame_rate_target.fps', controls.frameCap === 'stream' ? STREAM_FPS : controls.fps);
  }
  if (shown.has('chill') && controls.chill) {
    put('chill.enabled', true);
    put('chill.min_fps', controls.chillMin);
    put('chill.max_fps', controls.chillMax);
  }
  if (shown.has('sharpening') && controls.sharpening) {
    put('image_sharpening.enabled', true);
    put('image_sharpening.sharpness', controls.sharpness);
  }
  if (shown.has('anisotropic') && controls.anisotropic) put('anisotropic_filtering.enabled', true);
  if (shown.has('antiAliasing') && controls.antiAliasing !== 'game') {
    put('anti_aliasing.mode', controls.antiAliasing === 'enhance' ? 'enhance_app_settings' : 'override_app_settings');
    if (controls.antiAliasing === 'override') {
      put('anti_aliasing.level', controls.aaLevel);
      put('anti_aliasing.method', controls.aaMethod);
    }
  }
  if (shown.has('boost') && controls.boost) {
    put('boost.enabled', true);
    put('boost.min_resolution', controls.boostResolution);
  }
  return settings;
}

export interface RadeonChoice {
  profile: string;
  rows: SettingRow[];
}

/**
 * The profile and rows after a control changed: the preset's (or the list's) settings with the
 * change, as Custom, or the preset or Off they now equal. Raw rows are kept as they are.
 */
export function changeControls(profile: unknown, rows: SettingRow[], support: Support, next: Controls): RadeonChoice {
  const custom = profileOf(profile) === 'custom';
  const base = effectiveSettings(profile, rows);
  const { used } = controlsOf(base, support);
  const written = settingsOfControls(next, support);
  const raw = (custom ? rows : rowsOf(base)).filter((row) => !used.has(row.id.trim()) && !(row.id.trim() in written));
  const settings = { ...written, ...settingsOf(raw) };
  return { profile: profileFor(settings), rows: [...rowsOf(written), ...raw] };
}

/** The rows the controls do not show. A preset's are shown too, but only Custom's are edited. */
export function rawRows(profile: unknown, rows: SettingRow[], support: Support): SettingRow[] {
  const custom = profileOf(profile) === 'custom';
  const listed = custom ? rows : rowsOf(effectiveSettings(profile, rows));
  const { used } = controlsOf(settingsOf(listed), support);
  // A row with a problem stays in sight, even when a control shows its setting.
  return listed.filter((row) => !used.has(row.id.trim()) || (custom && rowError(row, rows) !== ''));
}

/** Picking a profile. Custom picked after a preset starts from the preset's settings. */
export function pickProfile(profile: unknown, rows: SettingRow[], next: string): RadeonChoice {
  const blank = rows.every((row) => !row.id.trim() && !row.value.trim());
  if (profileOf(next) === 'custom' && profileOf(profile) !== 'custom' && blank) {
    return { profile: next, rows: rowsOf(effectiveSettings(profile, rows)) };
  }
  return { profile: next, rows };
}

/** A new raw row, as Custom: a preset's settings are listed first. */
export function addRow(profile: unknown, rows: SettingRow[]): RadeonChoice {
  const listed = profileOf(profile) === 'custom' ? rows : rowsOf(effectiveSettings(profile, rows));
  return { profile: 'custom', rows: [...listed, settingRow()] };
}

/** A typed number as the setting takes it: whole and within its range, or null when blank. */
export function clampTo(setting: RadeonSetting | undefined, value: unknown): number | null {
  if (value === null || value === undefined || value === '') return null;
  const number = Math.round(Number(value));
  if (!Number.isFinite(number)) return null;
  const range = setting?.range;
  return range ? Math.min(range.max, Math.max(range.min, number)) : number;
}

/** The driver's current number for a setting, to start a control from. */
export function currentOf(setting: RadeonSetting | undefined): number | null {
  const value = setting?.value;
  return whole(value) ? clampTo(setting, value) : null;
}
