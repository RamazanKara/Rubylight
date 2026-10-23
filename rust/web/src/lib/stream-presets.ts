// The streaming presets: Lowest latency, Balanced and Reference quality. The
// table is the host's own (rust/core/src/stream_presets.json). build.ps1 builds
// a copy of rust/web, so the console keeps a byte-identical copy next to this
// file, and a host test fails if they differ.
import type { ConfigValue } from './api';
import type { Setting } from './settings-types';
import table from './stream_presets.json';

export interface Preset {
  id: string;
  label: string;
  description: string;
  settings: Readonly<Record<string, string | undefined>>;
}

export const SETTING = table.setting;
export const presets: readonly Preset[] = table.presets as readonly Preset[];
export const defaultPreset: string = table.default;
/** The settings a preset may set. */
export const covered: readonly string[] = table.covered.map((entry) => entry.key);

/** The name as the host's `find` reads it: trimmed, quotes dropped, lower case, `-` and space as `_`. */
function nameOf(value: ConfigValue | undefined): string {
  if (typeof value !== 'string') return '';
  return value.trim().replace(/^"+|"+$/g, '').trim().toLowerCase().replace(/[- ]/g, '_');
}

/** The id a stored or edited value means, as the host reads it; Balanced when unset or unknown. */
export function presetId(value: ConfigValue | undefined): string {
  const id = nameOf(value);
  return presets.some((preset) => preset.id === id) ? id : defaultPreset;
}

/**
 * What to tell the user when a stored or edited value is set but is not a
 * preset: the host loads it anyway and uses Balanced, so the console must not
 * quietly show Balanced as if that were what was saved. null when it is unset
 * or readable.
 */
export function presetNotice(value: ConfigValue | undefined): string | null {
  if (value === null || value === undefined) return null;
  if (typeof value === 'string' && nameOf(value) === '') return null;
  if (presets.some((preset) => preset.id === nameOf(value))) return null;
  const shown = typeof value === 'string' ? value.trim() : JSON.stringify(value);
  return `Saved stream preset ${shown} is not one of ${presets.map((preset) => preset.label).join(', ')}. The host uses ${presetById(defaultPreset).label} until you choose one.`;
}

export const presetById = (id: string): Preset =>
  presets.find((preset) => preset.id === id) ?? presets.find((preset) => preset.id === defaultPreset)!;

function asDefault(setting: Setting, text: string): ConfigValue {
  switch (setting.control.kind) {
    case 'toggle':
      return text === 'true';
    case 'number':
      return Number(text);
    default:
      return text;
  }
}

/**
 * The settings with the defaults the host uses under preset `id`: a covered
 * setting the preset lists has its value as the default, so a row shows what
 * is in effect and Reset goes back to the preset. Other settings are returned
 * as they are.
 */
export function withPreset(settings: readonly Setting[], id: string): Setting[] {
  const values = presetById(id).settings;
  return settings.map((setting) => {
    const value = values[setting.key];
    return value === undefined ? setting : { ...setting, default: asDefault(setting, value) };
  });
}
