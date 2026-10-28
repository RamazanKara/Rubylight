// Paired-device permissions and the editable form of a device's settings.
import { PERM, PERM_ALL, type Client } from '../../lib/api';
import { grantsValue, readGrants, type FolderGrants } from '../../lib/shared-folders';
import { SETTING as STREAM_PRESET, presets } from '../../lib/stream-presets';
import { settings as displaySettings } from '../../lib/schema/display';
import { settings as extensionSettings } from '../../lib/schema/extensions';

export const PERMISSION_GROUPS: { name: string; items: { bit: number; label: string }[] }[] = [
  {
    name: 'Apps',
    items: [
      { bit: PERM.LIST_APPS, label: 'List apps' },
      { bit: PERM.VIEW_STREAMS, label: 'View streams' },
      { bit: PERM.LAUNCH_APPS, label: 'Launch apps' },
    ],
  },
  {
    name: 'Input',
    items: [
      { bit: PERM.CONTROLLER, label: 'Controllers' },
      { bit: PERM.TOUCH, label: 'Touch' },
      { bit: PERM.PEN, label: 'Pen' },
      { bit: PERM.MOUSE, label: 'Mouse' },
      { bit: PERM.KEYBOARD, label: 'Keyboard' },
    ],
  },
  {
    name: 'Other',
    items: [
      { bit: PERM.CLIPBOARD_WRITE, label: 'Write to the clipboard' },
      { bit: PERM.CLIPBOARD_READ, label: 'Read the clipboard' },
      { bit: PERM.FILE_UPLOAD, label: 'Upload files (Experimental)' },
      { bit: PERM.FILE_DOWNLOAD, label: 'Download files (Experimental)' },
      { bit: PERM.SERVER_CMD, label: 'Host commands' },
    ],
  },
];

/** "List apps, Mouse, Keyboard" for a tooltip. */
export function grantedLabels(perm: number): string {
  return PERMISSION_GROUPS.flatMap((group) => group.items)
    .filter((item) => (perm & item.bit) === item.bit)
    .map((item) => item.label)
    .join(', ');
}

export const VIRTUAL_DISPLAY_MODES = [
  { value: 'disabled', label: 'Off, stream a monitor' },
  { value: 'per_client', label: 'One for each device' },
  { value: 'shared', label: 'One shared by all devices' },
];

export const VIRTUAL_DISPLAY_LAYOUTS = [
  { value: 'exclusive', label: 'Exclusive', hint: 'Turns the other monitors off while streaming.' },
  { value: 'extended', label: 'Extend — keep PC display primary', hint: 'Adds the virtual display beside your monitors and keeps the PC display as the main screen.' },
  { value: 'extended_primary', label: 'Extend — make virtual display primary', hint: 'The virtual display becomes the main screen, including the desktop and taskbar.' },
  {
    value: 'extended_isolated',
    label: 'Isolate — keep PC display primary',
    hint: 'Keeps the PC display primary and places the virtual display far away so the mouse cannot wander onto it.',
  },
  {
    value: 'extended_primary_isolated',
    label: 'Isolate — make virtual display primary',
    hint: 'The virtual display becomes the main screen, placed far from your other monitors.',
  },
];

export interface CommandRow {
  /** Keeps each row's inputs in place when another row is removed. */
  key: number;
  cmd: string;
  elevated: boolean;
}

/** The editor's state. An empty string means "use the host setting". */
export interface Draft {
  name: string;
  enabled: boolean;
  perm: number;
  output_name_override: string;
  /** WIDTHxHEIGHTxREFRESH, empty for the mode the device asks for. */
  display_mode: string;
  always_use_virtual_display: boolean;
  /** Keep the resolution picked in Windows during a stream for the next one. */
  remember_display_mode: boolean;
  /** The resolution kept that way, WIDTHxHEIGHT; shown, not edited. */
  remembered_resolution: string;
  /** The Windows scale kept the same way, in percent; shown, not edited. */
  remembered_scale: string;
  background_stream: boolean;
  virtual_display_mode: string;
  virtual_display_layout: string;
  /** The layout field was picked in the editor, so the host never learns over it. */
  virtual_display_layout_set_by_user: boolean;
  prefer_10bit_sdr: '' | 'on' | 'off';
  hdr_profile: string;
  allow_client_commands: boolean;
  /** The device's own stream preset id; empty follows the host's. An unknown saved name is kept as it is. */
  stream_preset: string;
  /** The preset exactly as stored (any spelling, any type), sent back while the choice is untouched. */
  stored_preset?: unknown;
  /** The device's display overrides (dd_rotation_option, dd_manual_rotation, dd_virtual_display_scale) as the selects show them; empty follows the host. */
  display: Record<DisplayOverride, string>;
  /** Those overrides exactly as stored, sent back while their choice is untouched. */
  stored_display: Partial<Record<DisplayOverride, unknown>>;
  /** The device's other config overrides, kept and sent back untouched. */
  other_overrides: Record<string, unknown>;
  /** Lets the device change Radeon settings and close games; off unless granted here. */
  host_control: boolean;
  /** Lets the device put the host to sleep while it is idle; off unless granted here. */
  allow_sleep: boolean;
  /** Lets the device attach its USB devices to the host; off unless granted here. */
  allow_usb: boolean;
  /** Read or write by shared folder id; a folder not named here is not shared with the device. */
  shared_folders: FolderGrants;
  do: CommandRow[];
  undo: CommandRow[];
}

/** Settings files written by older consoles hold "true" and "0" as often as booleans. */
function flag(value: unknown): boolean | null {
  if (value === true || value === 1 || value === 'true' || value === '1') return true;
  if (value === false || value === 0 || value === 'false' || value === '0') return false;
  return null;
}

function text(value: unknown): string {
  return typeof value === 'string' ? value.trim() : '';
}

function choice(value: unknown, allowed: { value: string }[]): string {
  const wanted = text(value).toLowerCase();
  return allowed.some((option) => option.value === wanted) ? wanted : '';
}

/** The host reads WIDTHxHEIGHTxREFRESH, with up to three decimals in the refresh. */
export function validDisplayMode(value: string): boolean {
  if (value.trim() === '') return true;
  const match = /^\s*(\d{1,5})[xX](\d{1,5})[xX](\d{1,4}(?:\.\d{1,3})?)\s*$/.exec(value);
  if (!match) return false;
  const [width, height, rate] = [Number(match[1]), Number(match[2]), Number(match[3])];
  return width >= 1 && width <= 16384 && height >= 1 && height <= 16384 && rate >= 1 && rate <= 1000;
}

let nextKey = 1;

export function commandRow(cmd = '', elevated = false): CommandRow {
  return { key: nextKey++, cmd, elevated };
}

function commandRows(value: unknown): CommandRow[] {
  if (!Array.isArray(value)) return [];
  return value.flatMap((entry: unknown) => {
    if (!entry || typeof entry !== 'object') return [];
    const { cmd, elevated } = entry as { cmd?: unknown; elevated?: unknown };
    const command = text(cmd);
    return command ? [commandRow(command, flag(elevated) === true)] : [];
  });
}

/** A saved preset name as an id the select offers; a name that is not a preset stays as written. */
function savedPreset(value: unknown): string {
  const name = text(value);
  const id = name.replace(/^"+|"+$/g, '').trim().toLowerCase().replace(/[- ]/g, '_');
  return presets.some((preset) => preset.id === id) ? id : name;
}

/** The display settings the device page sets for the device alone, in its config overrides. */
export const DISPLAY_OVERRIDES = ['dd_rotation_option', 'dd_manual_rotation', 'dd_virtual_display_scale'] as const;
export type DisplayOverride = (typeof DISPLAY_OVERRIDES)[number];

/** The choices each display override offers, as the host settings offer them. */
export const DISPLAY_OVERRIDE_OPTIONS = Object.fromEntries(
  DISPLAY_OVERRIDES.map((key) => {
    const control = [...displaySettings, ...extensionSettings].find((setting) => setting.key === key)?.control;
    if (control?.kind !== 'select') throw new Error(`${key} is not a select setting`);
    return [key, control.options.map(({ value, label }) => ({ value, label }))];
  }),
) as Record<DisplayOverride, { value: string; label: string }[]>;

/** A stored display override as its select shows it: a number as written, a choice in lower case. */
function shownDisplay(value: unknown): string {
  return typeof value === 'number' ? String(value) : text(value).toLowerCase();
}

export function toDraft(client: Client): Draft {
  const sdr = flag(client.prefer_10bit_sdr);
  const stored = client.config_overrides;
  const { [STREAM_PRESET]: preset, ...rest } =
    stored && typeof stored === 'object' && !Array.isArray(stored) ? (stored as Record<string, unknown>) : {};
  const other = { ...rest };
  const storedDisplay: Partial<Record<DisplayOverride, unknown>> = {};
  for (const key of DISPLAY_OVERRIDES) {
    if (key in other) {
      storedDisplay[key] = other[key];
      delete other[key];
    }
  }
  return {
    name: client.name,
    enabled: flag(client.enabled) !== false,
    perm: client.perm & PERM_ALL,
    output_name_override: text(client.output_name_override),
    display_mode: text(client.display_mode),
    always_use_virtual_display: flag(client.always_use_virtual_display) === true,
    remember_display_mode: flag(client.remember_display_mode) === true,
    remembered_resolution: text(client.remembered_resolution),
    remembered_scale: typeof client.remembered_scale === 'number' ? String(client.remembered_scale) : '',
    background_stream: flag(client.background_stream) === true,
    virtual_display_mode: choice(client.virtual_display_mode, VIRTUAL_DISPLAY_MODES),
    virtual_display_layout: choice(client.virtual_display_layout, VIRTUAL_DISPLAY_LAYOUTS),
    virtual_display_layout_set_by_user: flag(client.virtual_display_layout_set_by_user) === true,
    prefer_10bit_sdr: sdr === null ? '' : sdr ? 'on' : 'off',
    hdr_profile: text(client.hdr_profile),
    allow_client_commands: flag(client.allow_client_commands) !== false,
    stream_preset: savedPreset(preset),
    stored_preset: preset,
    display: Object.fromEntries(DISPLAY_OVERRIDES.map((key) => [key, shownDisplay(storedDisplay[key])])) as Record<
      DisplayOverride,
      string
    >,
    stored_display: storedDisplay,
    other_overrides: other,
    // As the host reads it: only true and "true" are on, so 1 and "yes" stay off.
    host_control: client.host_control === true || client.host_control === 'true',
    // Only a JSON true counts, as in Vibepollo.
    allow_sleep: client.allow_sleep === true,
    // As the host reads it (Client::allows_usb): only a JSON true.
    allow_usb: client.allow_usb === true,
    shared_folders: readGrants(client.shared_folders),
    do: commandRows(client.do),
    undo: commandRows(client.undo),
  };
}

type Command = { cmd: string; elevated: boolean };
export type Changes = Record<
  string,
  string | number | boolean | null | Command[] | Record<string, unknown>
>;

function commands(rows: CommandRow[]): Command[] {
  return rows.filter((row) => row.cmd.trim()).map((row) => ({ cmd: row.cmd.trim(), elevated: row.elevated }));
}

/**
 * The device's overrides: its other overrides plus its display overrides and preset; null when
 * there are none. A preset or display override the user did not touch goes back as it was stored,
 * so a value this console cannot show (another type, an unknown name) is not rewritten or lost by
 * saving something else.
 */
function overrides(draft: Draft): Record<string, unknown> | null {
  const untouched = draft.stored_preset !== undefined && savedPreset(draft.stored_preset) === draft.stream_preset;
  const preset = untouched ? { [STREAM_PRESET]: draft.stored_preset } : draft.stream_preset ? { [STREAM_PRESET]: draft.stream_preset } : {};
  const display: Record<string, unknown> = {};
  for (const key of DISPLAY_OVERRIDES) {
    const value = draft.display[key];
    if (key in draft.stored_display && shownDisplay(draft.stored_display[key]) === value) {
      display[key] = draft.stored_display[key];
    } else if (value !== '') {
      // The angle and the scale are numbers to the host; the option is a word.
      display[key] = key === 'dd_rotation_option' ? value : Number(value);
    }
  }
  const next = { ...draft.other_overrides, ...display, ...preset };
  return Object.keys(next).length > 0 ? next : null;
}

/** The draft as the host stores it: null clears a per-device setting. */
function serialise(draft: Draft): Changes {
  return {
    name: draft.name.trim(),
    enabled: draft.enabled,
    perm: draft.perm,
    output_name_override: draft.output_name_override || null,
    display_mode: draft.display_mode.trim() || null,
    always_use_virtual_display: draft.always_use_virtual_display,
    remember_display_mode: draft.remember_display_mode,
    // Turning it off forgets the kept resolution and scale.
    ...(draft.remember_display_mode ? {} : { remembered_resolution: null, remembered_scale: null }),
    background_stream: draft.background_stream,
    virtual_display_mode: draft.virtual_display_mode || null,
    virtual_display_layout: draft.virtual_display_layout || null,
    // Only a layout the user picked is theirs; one the host learned from a stream is not.
    virtual_display_layout_set_by_user: draft.virtual_display_layout && draft.virtual_display_layout_set_by_user ? true : null,
    prefer_10bit_sdr: draft.prefer_10bit_sdr === '' ? null : draft.prefer_10bit_sdr === 'on',
    hdr_profile: draft.hdr_profile || null,
    allow_client_commands: draft.allow_client_commands,
    config_overrides: overrides(draft),
    // The host takes only a boolean or null here; null removes the grant.
    host_control: draft.host_control ? true : null,
    allow_sleep: draft.allow_sleep ? true : null,
    allow_usb: draft.allow_usb ? true : null,
    // null removes every grant.
    shared_folders: grantsValue(draft.shared_folders),
    // The host rejects a null command list, so an empty list is sent instead.
    do: commands(draft.do),
    undo: commands(draft.undo),
  };
}

/** Whether `after` takes a folder away or leaves only reading where `before` could write. */
export function losesFolderAccess(before: FolderGrants, after: FolderGrants): boolean {
  return Object.entries(before).some(([id, access]) => !Object.hasOwn(after, id) || (access === 'write' && after[id] === 'read'));
}

/**
 * Whether saving ends the device's running stream. The host does that only when
 * the device is turned off, loses a permission bit, loses USB devices (its attached ones go
 * with the stream) or loses access to a shared folder; other edits, including Host control,
 * apply from the next request or stream.
 */
export function savingEndsStream(connected: boolean, before: Draft, after: Draft): boolean {
  return (
    connected &&
    ((before.enabled && !after.enabled) ||
      (before.perm & ~after.perm) !== 0 ||
      (before.allow_usb && !after.allow_usb) ||
      losesFolderAccess(before.shared_folders, after.shared_folders))
  );
}

/** Only the keys whose stored value would change. */
export function changes(before: Draft, after: Draft): Changes {
  const old = serialise(before);
  return Object.fromEntries(
    Object.entries(serialise(after)).filter(([key, value]) => JSON.stringify(value) !== JSON.stringify(old[key])),
  );
}
