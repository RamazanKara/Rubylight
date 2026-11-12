// The game Home's top shows: the one that runs, else the one the stores last saw played.
import type { App, SessionStatus, StreamSession } from '../../lib/api';
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

/** The live figures of the first running stream, for the hero; null when none runs. */
export function liveFigures(streams: StreamSession[]): {
  device: string;
  /** Other devices streaming at the same time. */
  more: number;
  /** Null until a frame was measured. */
  fps: number | null;
  mbps: number | null;
  /** One bitrate a second, the oldest first. */
  trend: number[];
} | null {
  const running = streams.filter((stream) => stream.state === 'RUNNING');
  const first = running[0];
  if (!first) return null;
  const perf = first.performance;
  const measured = perf.sample_frames > 0;
  return {
    device: first.device_name || 'a device',
    more: running.length - 1,
    fps: measured ? Math.round(perf.fps) : null,
    mbps: measured ? perf.bitrate_mbps : null,
    trend: perf.history.map((sample) => sample.bitrate_mbps),
  };
}
