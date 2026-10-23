// The streaming presets: Lowest latency, Balanced and Reference quality. The
// table is the host's own (rust/core/src/stream_presets.json), so the console
// and the host cannot disagree about what a preset sets.
import type { ConfigValue } from './api';
import type { Setting } from './settings-types';
import table from '../../../core/src/stream_presets.json';

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

/** The id a stored or edited value means, as the host reads it; Balanced when unset or unknown. */
export function presetId(value: ConfigValue | undefined): string {
  if (typeof value !== 'string') return defaultPreset;
  const id = value.trim().replace(/^"(.*)"$/, '$1').trim().toLowerCase().replace(/[- ]/g, '_');
  return presets.some((preset) => preset.id === id) ? id : defaultPreset;
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
