// Stored settings, edits, and the PATCH body built from them.
//
// The host keeps every value as text and GET /api/config parses each one as
// JSON when it can, so a toggle may arrive as true, "true", "enabled" or 1
// and a number as 20 or "20". Values are normalised for their control before
// they are shown or compared.
import type { Config, ConfigValue, PrepCommand } from '../../lib/api';
import type { Aliases, Setting } from '../../lib/settings-types';
import { readFolders, storedFolders } from '../../lib/shared-folders';

/** Keys GET /api/config adds that are not settings. They are never sent back. */
export const NOT_SETTINGS: ReadonlySet<string> = new Set(['status', 'platform', 'version']);

/** Edited values by key. null asks for the key to be removed (back to the default). */
export type Edits = Record<string, ConfigValue>;

/** Fields the console does not edit are kept: the host stores them too. */
export type Command = PrepCommand & { [field: string]: unknown };

export interface ServerCommand {
  name: string;
  cmd: string;
  elevated: boolean;
  [field: string]: unknown;
}

/** Read with the host's frame-rate parser: digits with at most three decimals. */
const RATES: ReadonlySet<string> = new Set(['frame_limiter_fps_limit', 'dd_manual_refresh_rate']);

export function settingsOnly(config: Config): Config {
  return Object.fromEntries(Object.entries(config).filter(([key]) => !NOT_SETTINGS.has(key)));
}

export const has = (record: object, key: string) => Object.hasOwn(record, key);

function unquote(text: string): string {
  const trimmed = text.trim();
  return trimmed.length >= 2 && trimmed.startsWith('"') && trimmed.endsWith('"') ? trimmed.slice(1, -1) : trimmed;
}

// The words the host's Config::boolean accepts.
const TRUE = new Set(['true', 'yes', '1', 'enable', 'enabled', 'on']);
const FALSE = new Set(['false', 'no', '0', 'disable', 'disabled', 'off']);

export function toBoolean(value: unknown): boolean | null {
  if (typeof value === 'boolean') return value;
  if (typeof value === 'number') return value === 1 ? true : value === 0 ? false : null;
  if (typeof value !== 'string') return null;
  const word = unquote(value).toLowerCase();
  if (TRUE.has(word)) return true;
  if (FALSE.has(word)) return false;
  return null;
}

/** Numbers, numeric text, quoted numbers ("756") and hexadecimal (0x2a), as the host reads them. */
export function toNumber(value: unknown): number | null {
  if (typeof value === 'number') return Number.isFinite(value) ? value : null;
  if (typeof value !== 'string') return null;
  const text = unquote(value);
  if (text === '') return null;
  if (/^[+-]?0x[0-9a-f]+$/i.test(text)) return parseInt(text, 16);
  const number = Number(text);
  return Number.isFinite(number) ? number : null;
}

/** Check an edited number before it can enter the settings PATCH. */
export function numberError(setting: Setting, text: string, badInput = false): string {
  if (setting.control.kind !== 'number') return '';
  if (badInput) return 'Enter a valid number.';
  // Clearing a number deliberately removes the override and uses the default.
  if (text.trim() === '') return '';
  const value = toNumber(text);
  if (value === null) return 'Enter a valid number.';
  if (setting.key === 'update_check_interval' && value > 0 && value < 3600) return 'Enter 0 or at least 3600 seconds.';
  if (setting.key === 'sleep_after_idle_minutes' && value > 0 && value < 5) return 'Enter 0 (off) or 5 to 1440 minutes.';
  const { min, max, step } = setting.control;
  if ((min !== undefined && value < min) || (max !== undefined && value > max)) {
    if (min !== undefined && max !== undefined) return `Enter a number from ${min} to ${max}.`;
    return min !== undefined ? `Enter ${min} or more.` : `Enter ${max} or less.`;
  }
  // Integer controls may use larger spinner increments. The host still
  // accepts any whole number, but discards fractions when parsing them.
  if (step !== undefined && step > 0 && Number.isInteger(step) && !Number.isInteger(value)) return 'Enter a whole number.';
  // The host rejects a fourth decimal (a fixed refresh rate then fails every stream).
  if (RATES.has(setting.key) && !/^\d+(\.\d{1,3})?$/.test(String(value))) return 'Enter at most three decimal places.';
  return '';
}

/** Check edited text the host parses strictly, before it can enter the PATCH. */
export function textError(setting: Setting, text: string): string {
  if (setting.key !== 'dd_manual_resolution' || text.trim() === '') return '';
  // config.rs splits on a lowercase x and checks the limits when a stream starts.
  const match = /^\s*(\d+)\s*x\s*(\d+)\s*$/.exec(text);
  if (!match) return 'Enter WIDTHxHEIGHT, for example 2560x1440.';
  const [width, height] = [Number(match[1]), Number(match[2])];
  if (width < 320 || width > 7680 || height < 200 || height > 4320) {
    return 'Enter a width from 320 to 7680 and a height from 200 to 4320.';
  }
  return '';
}

/** Strings as they are; numbers and booleans as written; anything else as JSON. */
export function toText(value: unknown): string {
  if (value === null || value === undefined) return '';
  if (typeof value === 'string') return value;
  if (typeof value === 'number' || typeof value === 'boolean') return String(value);
  return JSON.stringify(value);
}

/** A JSON array, or the "[a, b]" and "a, b" lists older consoles wrote. */
export function toList(value: unknown): string[] {
  if (Array.isArray(value)) return value.map((item: unknown) => toText(item));
  if (value === null || value === undefined) return [];
  if (typeof value !== 'string') return [toText(value)];
  const text = value.trim();
  if (text === '') return [];
  try {
    const parsed: unknown = JSON.parse(text);
    if (Array.isArray(parsed)) return toList(parsed);
  } catch {
    // Not JSON: a comma-separated list.
  }
  return text
    .replace(/^\[/, '')
    .replace(/\]$/, '')
    .split(',')
    .map(unquote)
    .filter((item) => item !== '');
}

function objects(value: unknown): Record<string, unknown>[] {
  let list = value;
  if (typeof list === 'string') {
    try {
      list = JSON.parse(list);
    } catch {
      return [];
    }
  }
  if (!Array.isArray(list)) return [];
  return list.filter(
    (item: unknown): item is Record<string, unknown> => !!item && typeof item === 'object' && !Array.isArray(item),
  );
}

export function toCommands(value: unknown): Command[] {
  return objects(value).map((entry) => ({
    ...entry,
    do: toText(entry.do),
    undo: toText(entry.undo),
    elevated: toBoolean(entry.elevated) === true,
  }));
}

export function toServerCommands(value: unknown): ServerCommand[] {
  return objects(value).map((entry) => ({
    ...entry,
    name: toText(entry.name),
    cmd: toText(entry.cmd),
    // stream.rs runs a host command elevated only for a JSON true.
    elevated: entry.elevated === true,
  }));
}

const COMPARE: Record<NonNullable<Aliases['match']>, (text: string) => string> = {
  exact: (text) => text,
  case: (text) => text.trim().toLowerCase(),
  dash: (text) => text.trim().toLowerCase().replaceAll('-', '_'),
  letters: (text) => text.replace(/[^a-z0-9]/gi, '').toLowerCase(),
};

/**
 * The select option the host reads `text` as: the option itself, or the one
 * an alias means. Anything else is returned as it is, and shows as a saved
 * custom choice.
 */
export function choice(setting: Setting, text: string): string {
  const { control, aliases } = setting;
  if (control.kind !== 'select' || !aliases || text.trim() === '') return text;
  if (control.options.some((option) => option.value === text)) return text;
  const compare = COMPARE[aliases.match ?? 'exact'];
  const wanted = compare(text);
  const option = control.options.find((candidate) => candidate.value !== '' && compare(candidate.value) === wanted);
  if (option) return option.value;
  for (const [spelling, value] of Object.entries(aliases.spellings ?? {})) {
    if (compare(spelling) === wanted) return value;
  }
  return text;
}

/** A PATCH value as the host reads it, for comparing: a select's alias is its option. */
function meaning(setting: Setting | undefined, value: ConfigValue): ConfigValue {
  return setting && typeof value === 'string' ? choice(setting, value) : value;
}

/** The value in the form its control edits. */
export function normalise(setting: Setting, value: ConfigValue | undefined): ConfigValue {
  switch (setting.control.kind) {
    case 'toggle':
      return toBoolean(value) ?? toBoolean(setting.default) ?? false;
    case 'select':
      return choice(setting, toText(value));
    case 'number':
      return toNumber(value);
    case 'list':
      return toList(value);
    case 'commands':
      return toCommands(value);
    case 'server-commands':
      return toServerCommands(value);
    case 'folders': {
      // An unreadable value stays as it is, so the editor can say why.
      const read = readFolders(value);
      return read.problem ? toText(value) : read.folders;
    }
    case 'json':
      return value === undefined ? null : value;
    default:
      return toText(value);
  }
}

/** The edited value, else the stored one, else the default. */
export function effective(setting: Setting, edits: Edits, stored: Config): ConfigValue {
  if (has(edits, setting.key)) return edits[setting.key] ?? setting.default;
  return stored[setting.key] ?? setting.default;
}

/** Every value as the host would see it after saving, defaults filled in; for visibleWhen. */
export function currentValues(settings: readonly Setting[], edits: Edits, stored: Config): Record<string, ConfigValue> {
  const values: Record<string, ConfigValue> = { ...stored };
  for (const [key, value] of Object.entries(edits)) {
    if (value === null) delete values[key];
    else values[key] = value;
  }
  for (const setting of settings) values[setting.key] = normalise(setting, effective(setting, edits, stored));
  return values;
}

const count = (n: number) => (n === 0 ? 'None' : n === 1 ? '1 command' : `${n} commands`);
const clip = (text: string, max = 60) => (text.length > max ? `${text.slice(0, max - 1)}…` : text);

export interface DefaultText {
  text: string;
  /** A raw value (set in mono), not a word such as On or Automatic. */
  literal: boolean;
}

/** 20%, 1000 ms. */
export const withUnit = (number: number, unit?: string) => (!unit ? String(number) : unit === '%' ? `${number}%` : `${number} ${unit}`);

const word = (text: string): DefaultText => ({ text, literal: false });
const literal = (text: string, otherwise: string): DefaultText =>
  text ? { text: clip(text), literal: true } : word(otherwise);

/** The default in words: On, an option's label, Automatic, Not set; or the value itself. */
export function describeDefault(setting: Setting): DefaultText {
  const control = setting.control;
  const value = setting.default;
  switch (control.kind) {
    case 'toggle':
      return word(toBoolean(value) ? 'On' : 'Off');
    case 'select': {
      const text = toText(value);
      const option = control.options.find((candidate) => candidate.value === text);
      return option ? word(option.label) : literal(text, 'Automatic');
    }
    case 'number': {
      const number = toNumber(value);
      if (number === null) return word('Not set');
      return literal(withUnit(number, control.unit), 'Not set');
    }
    case 'text':
      return literal(toText(value), 'Not set');
    case 'display':
    case 'adapter':
      return literal(toText(value), 'Automatic');
    case 'audio':
      return literal(toText(value), 'System default');
    case 'list':
      return literal(toList(value).join(', '), setting.key === 'hardware_metrics_selection' ? 'All supported readings' : 'None');
    case 'commands':
      return word(count(toCommands(value).length));
    case 'server-commands':
      return word(count(toServerCommands(value).length));
    case 'folders': {
      const n = readFolders(value).folders.length;
      return word(n === 0 ? 'None' : n === 1 ? '1 folder' : `${n} folders`);
    }
    case 'json':
      if (value === null || value === '') return word('Not set');
      if (Array.isArray(value)) return word(value.length ? `${value.length} entries` : 'None');
      // An object of empty lists, such as no mode substitutions.
      if (typeof value === 'object' && Object.values(value).every((item) => Array.isArray(item) && item.length === 0)) {
        return word('None');
      }
      return word('Built-in configuration');
  }
}

function isNonEmptyList(value: unknown): boolean {
  return toList(value).length > 0;
}

/** An emptied list is removed, unless the default has items: then [] is kept on purpose. */
function listValue(items: unknown[], fallback: ConfigValue): ConfigValue {
  return items.length || isNonEmptyList(fallback) ? items : null;
}

function textValue(value: unknown): string | null {
  const text = toText(value).trim();
  return text === '' ? null : text;
}

/**
 * The value as the host should store it. null means remove the key: empty
 * text, an emptied list, a cleared number. A key the schema does not
 * describe (setting undefined) is stored as the text typed.
 */
export function patchValue(setting: Setting | undefined, value: ConfigValue | undefined): ConfigValue {
  if (value === null || value === undefined) return null;
  if (!setting) return textValue(value);
  switch (setting.control.kind) {
    case 'toggle':
      return toBoolean(value);
    case 'number':
      return toNumber(value);
    case 'list':
      return listValue(
        toList(value)
          .map((item) => item.trim())
          .filter((item) => item !== ''),
        setting.default,
      );
    case 'commands':
      return listValue(
        toCommands(value)
          .map((command) => ({ ...command, do: command.do.trim(), undo: command.undo.trim() }))
          .filter((command) => command.do || command.undo),
        setting.default,
      );
    case 'server-commands':
      return listValue(
        toServerCommands(value)
          .map((command) => ({ ...command, name: command.name.trim(), cmd: command.cmd.trim() }))
          .filter((command) => command.name || command.cmd),
        setting.default,
      );
    case 'folders':
      return listValue(storedFolders(readFolders(value).folders), setting.default);
    case 'json':
      return value === '' ? null : value;
    default:
      return textValue(value);
  }
}

/** JSON with object keys sorted, so equal values compare equal. */
function stable(value: unknown): string {
  return (
    JSON.stringify(value, (_key, item: unknown) =>
      item && typeof item === 'object' && !Array.isArray(item)
        ? Object.fromEntries(Object.entries(item).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0)))
        : item,
    ) ?? 'undefined'
  );
}

export function sameValue(a: unknown, b: unknown): boolean {
  return stable(a) === stable(b);
}

/**
 * The PATCH body: only keys whose stored value would change, null for keys
 * to remove. An edit equal to what the host already uses is not a change,
 * so a stored alias is rewritten only when the edit means something else.
 */
export function computeChanges(
  settings: ReadonlyMap<string, Setting>,
  edits: Edits,
  stored: Config,
  explicit: ReadonlySet<string> = new Set(),
): Config {
  const changes: Config = {};
  for (const [key, edit] of Object.entries(edits)) {
    if (NOT_SETTINGS.has(key)) continue;
    const setting = settings.get(key);
    const isStored = has(stored, key);
    const next = edit === null ? null : patchValue(setting, edit);
    if (next === null) {
      if (isStored) changes[key] = null;
      continue;
    }
    // A key in `explicit` is stored even when it equals the default shown, so
    // a later change of that default (another preset) cannot take it over.
    const current = isStored
      ? patchValue(setting, stored[key])
      : setting && !explicit.has(key)
        ? patchValue(setting, setting.default)
        : null;
    if (!sameValue(meaning(setting, next), meaning(setting, current))) changes[key] = next;
  }
  return changes;
}

/**
 * Whether the host would use something other than the default once the
 * edits are saved. A cleared or unreadable value falls back to the default.
 */
export function differsFromDefault(setting: Setting, edits: Edits, stored: Config): boolean {
  const value = has(edits, setting.key) ? edits[setting.key] : stored[setting.key];
  const next = patchValue(setting, value);
  return next !== null && !sameValue(meaning(setting, next), meaning(setting, patchValue(setting, setting.default)));
}

/**
 * What a folded group's summary shows: how many settings differ from their default, how many have an unsaved
 * change, and whether the group must be open so that an unsaved change stays visible.
 */
export function foldStatus(
  settings: Setting[],
  edits: Edits,
  stored: Config,
  pending: ReadonlySet<string>,
): { customized: number; unsaved: number; mustOpen: boolean } {
  const customized = new Set(settings.filter(setting => differsFromDefault(setting, edits, stored)).map(setting => setting.key)).size;
  const unsaved = settings.filter(setting => pending.has(setting.key)).length;
  return { customized, unsaved, mustOpen: unsaved > 0 };
}

/** Whether the key has a value of its own once the edits are saved, whatever that value is. */
export function isSet(setting: Setting, edits: Edits, stored: Config): boolean {
  return patchValue(setting, has(edits, setting.key) ? edits[setting.key] : stored[setting.key]) !== null;
}

/** The stored values after the host accepted `changes`. */
export function applyChanges(stored: Config, changes: Config): Config {
  const next = { ...stored };
  for (const [key, value] of Object.entries(changes)) {
    if (value === null) delete next[key];
    else next[key] = value;
  }
  return next;
}

/** A JSON value as text for editing; empty when not set. */
export function jsonText(value: ConfigValue | undefined): string {
  if (value === null || value === undefined || value === '') return '';
  return JSON.stringify(value, null, 2);
}

export type Parsed = { ok: true; value: ConfigValue } | { ok: false; error: string };

/** Empty text is valid and means not set. */
export function parseJson(text: string): Parsed {
  if (text.trim() === '') return { ok: true, value: '' };
  try {
    return { ok: true, value: JSON.parse(text) as ConfigValue };
  } catch (error) {
    return { ok: false, error: error instanceof Error ? error.message : String(error) };
  }
}
