/**
 * The override cascade: which layer decides a stream setting for one game on one device.
 *
 * The host reads the layers in this order (`effective_config` in stream/config.rs): its own
 * settings, then the device's, then the game's, then the game's block for the device. A later
 * layer wins, so the chain reads winner first: game on device, game, device, host. The keys are
 * the ones the catalog marks `override_allowed` (rust/core/config/settings.toml).
 */
import { schema } from '../../lib/settings-schema';
import type { Setting } from '../../lib/settings-types';
import { OVERRIDE_ALLOWED } from '../../lib/schema/overrides.gen';
import { choice, sameValue, toBoolean, toNumber, toText, withUnit } from '../settings/values';

export type LayerId = 'pair' | 'app' | 'device' | 'host';

/** Winner first. */
export const LAYER_ORDER: readonly LayerId[] = ['pair', 'app', 'device', 'host'];

/** What a card says about the layer that decides. */
export const OWNER_TAG: Readonly<Record<LayerId, string>> = {
  pair: 'Custom for this device',
  app: 'App override',
  device: 'From Device',
  host: 'Inherited from Host',
};

export interface Layer {
  id: LayerId;
  /** As the chain names it: "Portal 2 on Steam Deck", "Portal 2", "Steam Deck", "Host". */
  name: string;
  /** The layer's stored overrides; the host's settings for the host. `null` while they are not read. */
  values: Readonly<Record<string, unknown>> | null | undefined;
}

export interface CascadeGroup {
  id: string;
  title: string;
  keys: readonly string[];
}

/** The stream settings a device or a game on a device can set for itself, by topic. */
export const CASCADE_GROUPS: readonly CascadeGroup[] = [
  {
    id: 'display',
    title: 'Display',
    keys: [
      'dd_resolution_option',
      'dd_manual_resolution',
      'dd_refresh_rate_option',
      'dd_manual_refresh_rate',
      'dd_hdr_option',
      'dd_activate_virtual_display',
      'virtual_display_refresh',
    ],
  },
  {
    id: 'stream',
    title: 'Stream and codec',
    keys: [
      'max_bitrate',
      'fec_percentage',
      'hevc_mode',
      'av1_mode',
      'packed_444',
      'frame_limiter_enable',
      'frame_limiter_provider',
      'frame_limiter_fps_limit',
    ],
  },
  { id: 'sound', title: 'Sound', keys: ['audio_capture_mode', 'virtual_sink', 'stream_mic', 'audio_surround_714'] },
  { id: 'input', title: 'Input', keys: ['mouse_mode', 'controller'] },
];

export const CASCADE_KEYS: readonly string[] = CASCADE_GROUPS.flatMap((group) => group.keys);

const overridable: ReadonlySet<string> = new Set(OVERRIDE_ALLOWED);
const byKey = new Map(schema.settings.map((setting) => [setting.key, setting]));

/** The catalog's setting for a cascade key; `undefined` for a key the host does not let a layer override. */
export function cascadeSetting(key: string): Setting | undefined {
  return overridable.has(key) ? byKey.get(key) : undefined;
}

/** Whether a layer sets the value: the host skips an absent, null or empty override (`apply_overrides`). */
export const isSet = (value: unknown): boolean => value !== undefined && value !== null && value !== '';

export interface Step {
  layer: Layer;
  set: boolean;
  value: unknown;
  /** The layer that decides. */
  wins: boolean;
}

export interface Resolved {
  owner: LayerId;
  value: unknown;
  /** Winner first, the host last. */
  steps: Step[];
  /** The host's settings are not read yet, and no layer above it decides. */
  unknown: boolean;
}

/** Which layer decides `setting` among `layers` (winner first; the host, if listed, always has a value). */
export function resolve(setting: Setting, layers: readonly Layer[]): Resolved {
  const steps = layers.map((layer): Step => {
    const stored = layer.values?.[setting.key];
    if (layer.id === 'host') return { layer, set: true, value: isSet(stored) ? stored : setting.default, wins: false };
    return { layer, set: isSet(stored), value: stored, wins: false };
  });
  const winner = steps.find((step) => step.set) ?? steps[steps.length - 1];
  if (winner) winner.wins = true;
  return {
    owner: winner?.layer.id ?? 'host',
    value: winner?.value,
    steps,
    unknown: winner?.layer.id === 'host' && winner.layer.values === null,
  };
}

/** A stored value in words: an option's label, On or Off, a number with its unit. */
export function shown(setting: Setting, value: unknown): string {
  const { control } = setting;
  if (!isSet(value)) return 'Not set';
  if (control.kind === 'toggle') {
    const on = toBoolean(value);
    return on === null ? toText(value) : on ? 'On' : 'Off';
  }
  if (control.kind === 'select') {
    const text = choice(setting, toText(value));
    return control.options.find((option) => option.value === text)?.label ?? text;
  }
  if (control.kind === 'number') {
    if (setting.defaultText && sameValue(value, setting.default)) return setting.defaultText;
    const number = toNumber(value);
    if (number === null) return toText(value);
    return control.presets?.find((preset) => preset.value === number)?.label ?? withUnit(number, control.unit);
  }
  const text = toText(value).trim();
  if (control.kind === 'text' && control.presets) return control.presets.find((preset) => preset.value === text)?.label ?? text;
  return text;
}

/** A layer's stored value as its control holds it: `''` is "not set here". */
export function controlText(setting: Setting, value: unknown): string {
  if (!isSet(value)) return '';
  if (setting.control.kind === 'toggle') {
    const on = toBoolean(value);
    return on === null ? toText(value) : on ? 'on' : 'off';
  }
  if (setting.control.kind === 'select') return choice(setting, toText(value));
  return toText(value);
}

/** What a control's text stores; `undefined` clears the layer's value, so the next layer decides. */
export function storedValue(setting: Setting, text: string): string | number | boolean | undefined {
  const value = text.trim();
  if (value === '') return undefined;
  if (setting.control.kind === 'toggle') return value === 'on';
  if (setting.control.kind === 'number') {
    const number = Number(value);
    return Number.isFinite(number) ? number : undefined;
  }
  return value;
}

/** Why a typed number is not one the host takes, or `''`. */
export function rangeError(setting: Setting, text: string): string {
  const { control } = setting;
  const value = text.trim();
  if (control.kind !== 'number' || value === '') return '';
  const number = Number(value);
  if (!Number.isFinite(number)) return 'Enter a number.';
  if (control.min !== undefined && number < control.min) return `At least ${withUnit(control.min, control.unit)}.`;
  if (control.max !== undefined && number > control.max) return `At most ${withUnit(control.max, control.unit)}.`;
  return '';
}

/** How many of `keys` the layer sets. */
export function setCount(keys: readonly string[], values: Readonly<Record<string, unknown>> | null | undefined): number {
  return keys.filter((key) => isSet(values?.[key])).length;
}
