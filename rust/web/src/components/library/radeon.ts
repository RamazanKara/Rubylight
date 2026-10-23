// A game's Radeon profile: the app's own `radeon-profile` and, for Custom, the Radeon
// settings it lists in `radeon-settings`. The host checks both when the app is saved
// (rust/core/src/radeon_profile.rs); the checks here only spare a round trip.
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

const ID = /^[a-z0-9_]+(\.[a-z0-9_]+)+$/;

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
