// The game Home's top shows: the one that runs, else the one the stores last saw played.
import type { App, SessionStatus } from '../../lib/api';
import { isRunningApp } from '../library/app';

export interface HeroGame {
  /** The library app, when the game is one; null for an app no longer in the library. */
  app: App | null;
  name: string;
  /** Runs on this PC (`paused` with no device connected), or was played last. */
  state: 'running' | 'paused' | 'last';
}

/** The app the stores saw played most recently; null when none says. */
export function lastPlayed(apps: App[]): App | null {
  let best: App | null = null;
  for (const app of apps) {
    const at = app['last-played'];
    if (typeof at === 'number' && at > 0 && at > (best?.['last-played'] ?? 0)) best = app;
  }
  return best;
}

export function heroGame(apps: App[] | null, status: SessionStatus | null): HeroGame | null {
  if (status?.appRunning) {
    const name = status.app?.name || status.appName || 'Unnamed app';
    const app = apps?.find((app) => isRunningApp(status, app)) ?? null;
    return { app, name, state: status.paused ? 'paused' : 'running' };
  }
  const last = apps ? lastPlayed(apps) : null;
  return last ? { app: last, name: last.name, state: 'last' } : null;
}

/**
 * What Home tells a player whose device is paired and no stream runs: where a
 * game starts. The device holds the game list, so the console can only name
 * the way in.
 */
export function readyCopy(hostName: string | undefined): string {
  return `Open Nightjar or Moonlight on your device and choose ${listedAs(hostName)} to play.`;
}

/** The same for the device that paired just now, under the first-run hero. */
export function pairedCopy(deviceName: string, hostName: string | undefined): string {
  return `${deviceName} is paired. Open Nightjar or Moonlight on it and choose ${listedAs(hostName)} to play.`;
}

/** The PC's name as the client lists it: never broken between its own words. */
function listedAs(hostName: string | undefined): string {
  return (hostName?.trim() || 'this PC').replace(/ /g, '\u00A0');
}
