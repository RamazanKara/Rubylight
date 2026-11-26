/**
 * Quick Suspend on Home: what the console says about a paused game and its buttons' wording.
 * The host decides everything (`/api/session/suspend`); this only words its answer.
 */
import type { QuickSuspendStatus } from './api';

/** Home offers Suspend game only when the host says the running game can be paused now. */
export function canSuspend(quick: QuickSuspendStatus | null): boolean {
  return quick?.state === 'ready';
}

/** The game is paused: Home shows the banner with Resume game. */
export function isSuspended(quick: QuickSuspendStatus | null): boolean {
  return quick?.state === 'suspended' || quick?.state === 'resuming';
}

/** The banner's line: what is paused, since when and from where. */
export function suspendedCopy(quick: QuickSuspendStatus, now = Date.now()): string {
  const title = quick.app_title || 'The game';
  const since = quick.suspended_since_unix_ms ? sinceText(quick.suspended_since_unix_ms, now) : '';
  const by = quick.suspended_by ? ` from ${quick.suspended_by}` : '';
  return `${title} is paused on this PC${since ? ` ${since}` : ''}${by}. It stays paused until you resume it here or from the device that paused it, a device with Host control resumes it, or a stream of it starts.`;
}

function sinceText(unixMs: number, now: number): string {
  const minutes = Math.max(0, Math.round((now - unixMs) / 60_000));
  if (minutes < 2) return 'since a moment ago';
  if (minutes < 120) return `for ${minutes} min`;
  return `since ${new Date(unixMs).toLocaleString()}`;
}

/** What a suspend or resume answered, for the toast: a refusal's reason, or the new state. */
export function outcome(quick: QuickSuspendStatus, asked: 'suspend' | 'resume'): { text: string; ok: boolean } {
  if (quick.state === 'refused' || (asked === 'suspend' && quick.state !== 'suspended')) {
    return { text: quick.message || 'Rubylight could not pause the game.', ok: false };
  }
  const title = quick.app_title || 'the game';
  return asked === 'suspend'
    ? { text: `Paused ${title}.`, ok: true }
    : { text: `Resumed ${title}.`, ok: quick.state !== 'suspended' };
}
