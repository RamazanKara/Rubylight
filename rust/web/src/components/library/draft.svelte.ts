// The editable copy of an app. The host replaces the whole record on save, so
// every key it sent is kept, including the ones this editor never shows.
import type { App, PrepCommand } from '../../lib/api';
import { GAME_FILES_KEY, gameFileEntries, gameFileRows, gameFilesError, type GameFileRow } from './gamefiles';
import { PROFILE_KEY, SETTINGS_KEY, profileError, profileOf, rowsOf, settingsOf, type SettingRow } from './radeon';

export interface PrepRow extends PrepCommand {
  /** Keeps each row's inputs in place when rows move. */
  key: number;
}
export interface TextRow {
  key: number;
  value: string;
}

let nextKey = 1;
export const prepRow = (command: Partial<PrepCommand> = {}): PrepRow => ({
  key: nextKey++,
  do: typeof command.do === 'string' ? command.do : '',
  undo: typeof command.undo === 'string' ? command.undo : '',
  elevated: command.elevated === true,
});
export const textRow = (value = ''): TextRow => ({ key: nextKey++, value });

/** Switches, with the value the host uses when the key is missing. */
export const FLAGS = {
  elevated: false,
  'auto-detach': true,
  'wait-all': true,
  'allow-client-commands': true,
  'exclude-global-state-cmd': false,
  'exclude-global-prep-cmd': false,
  'virtual-display': false,
  'virtual-display-primary': false,
  'use-app-identity': false,
  'per-client-app-identity': false,
  'gen1-framegen-fix': false,
  'frame-gen-limiter-fix': false,
  'lossless-scaling-enabled': false,
  'lossless-scaling-legacy-auto-detect': false,
  'background-stream': false,
  /** The game's "<name> (Co-op)" entry lists in the library. */
  'coop-entry': true,
} as const;
export type FlagKey = keyof typeof FLAGS;

/** Whole numbers and the range the host accepts. A missing key uses the host's value. */
export const NUMBERS: Record<string, { min: number; max?: number }> = {
  'exit-timeout': { min: 0, max: 300 },
  'scale-factor': { min: 1 },
  'lossless-scaling-target-fps': { min: 1, max: 480 },
  'lossless-scaling-rtss-limit': { min: 1, max: 480 },
  'lossless-scaling-launch-delay': { min: 0, max: 600 },
};
export type NumberKey =
  | 'exit-timeout'
  | 'scale-factor'
  | 'lossless-scaling-target-fps'
  | 'lossless-scaling-rtss-limit'
  | 'lossless-scaling-launch-delay';

/** Choices and free text. An empty value removes the key. */
export type TextKey =
  | 'virtual-display-mode'
  | 'virtual-display-layout'
  | 'display-output'
  | 'dd-configuration-option'
  | 'frame-generation-mode'
  | 'gamepad'
  | 'radeon-profile'
  | 'lossless-scaling-profile';
export function blankApp(): App {
  return { name: '', cmd: '', 'working-dir': '', 'prep-cmd': [] };
}

/** JSON with object keys sorted, for comparing records. */
export function stable(value: unknown): string {
  return JSON.stringify(value, (_key, item: unknown) =>
    item && typeof item === 'object' && !Array.isArray(item)
      ? Object.fromEntries(Object.entries(item).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0)))
      : item,
  );
}

/** Read a switch the way the host does. */
function flagValue(value: unknown, fallback: boolean): boolean {
  if (typeof value === 'boolean') return value;
  if (typeof value === 'string') return ['true', '1', 'yes'].includes(value);
  return fallback;
}

function prepRows(value: unknown): PrepRow[] {
  if (!Array.isArray(value)) return [];
  return value.flatMap((entry: unknown) =>
    entry && typeof entry === 'object' ? [prepRow(entry as Partial<PrepCommand>)] : [],
  );
}

function textRows(value: unknown): TextRow[] {
  if (!Array.isArray(value)) return [];
  return value.flatMap((entry: unknown) => (typeof entry === 'string' ? [textRow(entry)] : []));
}

/** The app's settings for single devices, by device id: each block holds the app's own keys. */
export const DEVICES_KEY = 'device-overrides';
export const CONFIG_KEY = 'config-overrides';
/** The general settings the editor offers for one device. */
export const STREAM_PRESET = 'stream_preset';
export const FRAME_LIMIT = 'frame_limiter_fps_limit';

type Block = Record<string, unknown>;
const isObject = (value: unknown): value is Block => !!value && typeof value === 'object' && !Array.isArray(value);

function blocksOf(app: App): Block {
  const blocks = app[DEVICES_KEY];
  return isObject(blocks) ? blocks : {};
}

function deviceFileRows(app: App): Record<string, GameFileRow[]> {
  return Object.fromEntries(
    Object.entries(blocksOf(app)).flatMap(([device, block]) =>
      isObject(block) ? [[device, gameFileRows(block[GAME_FILES_KEY])]] : [],
    ),
  );
}

/** The record to send: blank command rows are dropped. */
function assemble(
  app: App,
  prep: PrepRow[],
  detached: TextRow[],
  keepDetached: boolean,
  radeon: SettingRow[],
  files: GameFileRow[],
  deviceFiles: Record<string, GameFileRow[]>,
): App {
  const record: App = { ...app };
  record['prep-cmd'] = prep
    .filter((row) => row.do.trim() || row.undo.trim())
    .map(({ do: run, undo, elevated }) => ({ do: run, undo, elevated }));
  const commands = detached.map((row) => row.value).filter((value) => value.trim());
  if (commands.length || keepDetached) record.detached = commands;
  else delete record.detached;
  // Only a Custom profile lists settings; the host refuses them beside any other.
  const settings = settingsOf(radeon);
  if (profileOf(record[PROFILE_KEY]) === 'custom' && Object.keys(settings).length) record[SETTINGS_KEY] = settings;
  else delete record[SETTINGS_KEY];
  const entries = gameFileEntries(files);
  if (entries.length) record[GAME_FILES_KEY] = entries;
  else delete record[GAME_FILES_KEY];
  // A device's block with nothing set is not kept. Blocks of devices that are not paired are
  // kept as they are; the host ignores them.
  if (isObject(record[DEVICES_KEY])) {
    const blocks: Block = {};
    for (const [device, stored] of Object.entries(blocksOf(record))) {
      if (!isObject(stored)) {
        blocks[device] = stored;
        continue;
      }
      const block = { ...stored };
      const rows = deviceFiles[device];
      if (rows) {
        const listed = gameFileEntries(rows);
        if (listed.length) block[GAME_FILES_KEY] = listed;
        else delete block[GAME_FILES_KEY];
      }
      const config = block[CONFIG_KEY];
      if (isObject(config) && !Object.keys(config).length) delete block[CONFIG_KEY];
      if (Object.keys(block).length) blocks[device] = block;
    }
    if (Object.keys(blocks).length) record[DEVICES_KEY] = blocks as App[typeof DEVICES_KEY];
    else delete record[DEVICES_KEY];
  }
  return record;
}

function normalise(source: App): App {
  const copy = $state.snapshot(source) as App;
  return assemble(
    copy,
    prepRows(copy['prep-cmd']),
    textRows(copy.detached),
    'detached' in copy,
    rowsOf(copy[SETTINGS_KEY]),
    gameFileRows(copy[GAME_FILES_KEY]),
    deviceFileRows(copy),
  );
}

/** What is wrong with a device's frame limit, or ''. The host reads at most three decimals. */
export function frameLimitError(value: unknown): string {
  if (value === undefined || value === null || value === '') return '';
  const text = String(value).trim();
  return /^\d{1,4}(\.\d{1,3})?$/.test(text) && Number(text) <= 1000 ? '' : 'Enter 0 to 1000, with at most three decimals.';
}

export class AppDraft {
  /** Everything but the command lists, which live in the rows below. */
  app = $state<App>(blankApp());
  prep = $state<PrepRow[]>([]);
  detached = $state<TextRow[]>([]);
  /** The settings of a Custom Radeon profile. */
  radeon = $state<SettingRow[]>([]);
  /** The app's managed game files. */
  files = $state<GameFileRow[]>([]);
  /** Each device block's managed game files, by device id. */
  deviceFiles = $state<Record<string, GameFileRow[]>>({});
  #saved = $state.raw<App>(blankApp());
  #baseline = $state('');

  /** Why the Radeon profile cannot be saved, or ''. */
  readonly radeonError = $derived(profileError(this.app[PROFILE_KEY], this.radeon));
  /** Why the game files cannot be saved, or ''. */
  readonly filesError = $derived(gameFilesError(this.files));
  /** Why a device's settings cannot be saved, or ''. */
  readonly devicesError = $derived(
    this.devices()
      .map((device) => frameLimitError(this.deviceSetting(device, FRAME_LIMIT)) || gameFilesError(this.deviceFiles[device] ?? []))
      .find(Boolean) ?? '',
  );
  readonly changed = $derived(stable(this.record()) !== this.#baseline);
  readonly nameMissing = $derived(this.app.name.trim() === '');
  readonly invalid = $derived(
    this.nameMissing ||
      this.radeonError !== '' ||
      this.filesError !== '' ||
      this.devicesError !== '' ||
      Object.keys(NUMBERS).some((key) => this.numberError(key as NumberKey) !== ''),
  );

  constructor(source: App = blankApp()) {
    this.reset(source);
  }

  /** The record as the host has it. */
  get saved(): App {
    return this.#saved;
  }

  reset(source: App) {
    const copy = $state.snapshot(source) as App;
    this.#saved = copy;
    this.app = $state.snapshot(source) as App;
    this.prep = prepRows(copy['prep-cmd']);
    this.detached = textRows(copy.detached);
    this.radeon = rowsOf(copy[SETTINGS_KEY]);
    this.files = gameFileRows(copy[GAME_FILES_KEY]);
    this.deviceFiles = deviceFileRows(copy);
    this.#baseline = stable(normalise(copy));
  }

  /** The host now has `record`; later edits stay unsaved. */
  markSaved(record: App) {
    this.#saved = $state.snapshot(record) as App;
    if (record.uuid) this.app.uuid = record.uuid;
    this.#baseline = stable(normalise(record));
  }

  /** Whether `source` is what this draft started from. */
  matches(source: App): boolean {
    return stable(normalise(source)) === this.#baseline;
  }

  record(): App {
    return assemble(
      $state.snapshot(this.app) as App,
      this.prep,
      this.detached,
      'detached' in this.#saved,
      $state.snapshot(this.radeon) as SettingRow[],
      $state.snapshot(this.files) as GameFileRow[],
      $state.snapshot(this.deviceFiles) as Record<string, GameFileRow[]>,
    );
  }

  /** The devices the app has its own settings for, paired or not. */
  devices(): string[] {
    return Object.keys(blocksOf(this.app));
  }
  addDevice(device: string) {
    if (!isObject(this.app[DEVICES_KEY])) this.app[DEVICES_KEY] = {};
    const blocks = this.app[DEVICES_KEY] as Block;
    if (!isObject(blocks[device])) blocks[device] = {};
    this.deviceFiles[device] ??= [];
  }
  removeDevice(device: string) {
    const blocks = this.app[DEVICES_KEY];
    if (isObject(blocks)) delete blocks[device];
    delete this.deviceFiles[device];
  }
  #block(device: string): Block | null {
    const block = blocksOf(this.app)[device];
    return isObject(block) ? block : null;
  }
  /** A general setting of the app on `device`; undefined when the app's own applies. */
  deviceSetting(device: string, key: string): unknown {
    const config = this.#block(device)?.[CONFIG_KEY];
    return isObject(config) ? config[key] : undefined;
  }
  setDeviceSetting(device: string, key: string, value: string | number | boolean | undefined) {
    const block = this.#block(device);
    if (!block) return;
    if (!isObject(block[CONFIG_KEY])) block[CONFIG_KEY] = {};
    const config = block[CONFIG_KEY] as Block;
    if (value === undefined || value === '') delete config[key];
    else config[key] = value;
  }
  /** The Radeon profile on `device`: null when the app's applies, '' for Off. */
  deviceRadeon(device: string): string | null {
    const value = this.#block(device)?.[PROFILE_KEY];
    return typeof value === 'string' ? value : null;
  }
  setDeviceRadeon(device: string, profile: string | null) {
    const block = this.#block(device);
    if (!block) return;
    if (profile === null) {
      delete block[PROFILE_KEY];
      delete block[SETTINGS_KEY];
      return;
    }
    // A preset brings its own settings; only a Custom profile lists them.
    if (profileOf(profile) !== 'custom') delete block[SETTINGS_KEY];
    block[PROFILE_KEY] = profile;
  }

  /** The cover file's path on the host; '' for none. */
  cover(): string {
    const value = this.app['image-path'];
    return typeof value === 'string' ? value : '';
  }
  setCover(path: string) {
    if (path) this.app['image-path'] = path;
    else delete this.app['image-path'];
  }

  flag(key: FlagKey): boolean {
    return flagValue(this.app[key], FLAGS[key]);
  }
  setFlag(key: FlagKey, value: boolean) {
    // The host's default needs no key, unless the record already spelled it out.
    if (value === FLAGS[key] && !(key in this.#saved)) delete this.app[key];
    else this.app[key] = value;
  }

  /** '' when the host setting applies. */
  tristate(key: 'prefer-10bit-sdr' | 'terminate-on-pause'): '' | 'on' | 'off' {
    const value = this.app[key];
    if (key === 'terminate-on-pause' && typeof value === 'string') {
      return flagValue(value, false) ? 'on' : 'off';
    }
    if (value === true || value === 'true' || value === '1' || value === 'on') return 'on';
    if (value === false || value === 'false' || value === '0' || value === 'off') return 'off';
    return '';
  }
  setTristate(key: 'prefer-10bit-sdr' | 'terminate-on-pause', value: '' | 'on' | 'off') {
    if (value === '') delete this.app[key];
    else this.app[key] = value === 'on';
  }

  text(key: TextKey): string {
    const value = this.app[key];
    return typeof value === 'string' ? value : '';
  }
  setText(key: TextKey, value: string) {
    if (value.trim()) this.app[key] = value;
    else delete this.app[key];
  }

  number(key: NumberKey): number | undefined {
    const value = this.app[key];
    if (typeof value === 'number') return value;
    if (typeof value === 'string' && value.trim() !== '' && Number.isFinite(Number(value))) return Number(value);
    return undefined;
  }
  setNumber(key: NumberKey, value: number | null | undefined) {
    if (value === null || value === undefined || Number.isNaN(value)) delete this.app[key];
    else this.app[key] = value;
  }
  numberError(key: NumberKey): string {
    const value = this.number(key);
    const range = NUMBERS[key];
    if (value === undefined || !range) return '';
    if (!Number.isInteger(value)) return 'Enter a whole number.';
    if (range.max === undefined) return value < range.min ? `Enter ${range.min} or more.` : '';
    return value < range.min || value > range.max ? `Enter ${range.min} to ${range.max}.` : '';
  }

  /** The Lossless Scaling profile the app uses. */
  losslessProfile(): 'lossless-scaling-recommended' | 'lossless-scaling-custom' {
    return this.text('lossless-scaling-profile').toLowerCase() === 'recommended'
      ? 'lossless-scaling-recommended'
      : 'lossless-scaling-custom';
  }
  /** A value in the app's Lossless Scaling profile. */
  lossless(field: string): unknown {
    const profile = this.app[this.losslessProfile()];
    return profile && typeof profile === 'object' ? (profile as Record<string, unknown>)[field] : undefined;
  }
  setLossless(field: string, value: unknown) {
    const key = this.losslessProfile();
    const current = this.app[key];
    const profile = { ...(current && typeof current === 'object' ? (current as Record<string, unknown>) : {}) };
    if (value === undefined || value === '' || (typeof value === 'number' && Number.isNaN(value))) delete profile[field];
    else profile[field] = value;
    if (Object.keys(profile).length) this.app[key] = profile;
    else delete this.app[key];
  }
}
