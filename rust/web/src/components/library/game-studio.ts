/**
 * What the game page's header says about a game's stream profile: the settings the saved app
 * carries of its own, as short badges. Nothing is guessed: a badge names a stored value.
 */
import type { App } from '../../lib/api';
import { presets } from '../../lib/stream-presets';
import { PROFILE_KEY, profileOf, profiles } from './radeon';

export interface StudioFact {
  text: string;
  tone: 'neutral' | 'warn' | 'accent';
}

const record = (value: unknown): Record<string, unknown> | undefined =>
  value && typeof value === 'object' && !Array.isArray(value) ? (value as Record<string, unknown>) : undefined;

/** The saved app's own stream profile; one neutral badge when it has none. */
export function studioFacts(app: App): StudioFact[] {
  const facts: StudioFact[] = [];
  if (app['not-installed'] === true) facts.push({ text: 'Not installed', tone: 'warn' });
  const preset = record(app['config-overrides'])?.stream_preset;
  if (typeof preset === 'string' && preset) {
    facts.push({ text: `${presets.find((known) => known.id === preset)?.label ?? preset} preset`, tone: 'neutral' });
  }
  const driver = profileOf(app[PROFILE_KEY]);
  const driverLabel = driver ? profiles.find((known) => known.id === driver)?.label : undefined;
  if (driverLabel) facts.push({ text: `Driver preset: ${driverLabel}`, tone: 'neutral' });
  if (app['capture-window'] === 'window') facts.push({ text: 'Streams its window', tone: 'neutral' });
  const devices = Object.keys(record(app['device-overrides']) ?? {}).length;
  if (devices) facts.push({ text: `${devices} device ${devices === 1 ? 'profile' : 'profiles'}`, tone: 'accent' });
  if (!facts.length) facts.push({ text: 'Host stream settings', tone: 'neutral' });
  return facts;
}
