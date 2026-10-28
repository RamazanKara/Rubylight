import type { ChildSessionPhase, ChildSessionStatus } from '../../lib/api';

/** What the child-session viewer's phase means to the person at the console. */
export const phases: Record<ChildSessionPhase, { label: string; tone: 'neutral' | 'ok' | 'warn' | 'danger' }> = {
  idle: { label: 'Not attached', tone: 'neutral' },
  needs_password: { label: 'Needs your Windows password', tone: 'warn' },
  connecting: { label: 'Signing in', tone: 'neutral' },
  connected: { label: 'Attached', tone: 'ok' },
  reconnecting: { label: 'Reconnecting', tone: 'warn' },
  signing_out: { label: 'Signing out', tone: 'neutral' },
  sign_in_failed: { label: 'Sign-in failed', tone: 'danger' },
  signed_out: { label: 'Signed out', tone: 'neutral' },
};

/**
 * Which part of the panel applies: `off` until Background mode asks for a
 * child session, `refused` when it asks for one this PC can't run, `inside`
 * in the child session's own host, `unavailable` in
 * a host that can't check or change the settings (no administrator rights),
 * `setup` while Windows settings are missing (or could not be read), `ready`
 * to start.
 */
export function step(status: ChildSessionStatus): 'off' | 'refused' | 'inside' | 'unavailable' | 'setup' | 'ready' {
  if (status.mode === 'inside_child_session') return 'inside';
  if (status.mode === 'nucleus') return 'off';
  if (status.mode === 'unavailable') return 'refused';
  if (status.unavailable) return 'unavailable';
  return status.missing?.length === 0 ? 'ready' : 'setup';
}

/**
 * Whether Help shows the panel: while Background mode uses a child session,
 * while Windows settings wait to be restored, or while something went wrong.
 * Off and never set up, there is nothing to fix.
 */
export function pending(status: ChildSessionStatus): boolean {
  return step(status) !== 'off' || status.set_up || Boolean(status.viewer || status.problem || status.missing_error || status.display_error);
}

/**
 * The panel's status loads. A poll tick is skipped while a load is in flight
 * (the host reads the firewall, which can take longer than a tick), and only
 * the newest load's answer is applied, so a slow one never overwrites it.
 */
export function statusLoader<T>(fetch: () => Promise<T>, apply: (value: T) => void, fail: (error: unknown) => void) {
  let inFlight = 0;
  let newest = 0;
  async function load() {
    const ticket = ++newest;
    inFlight++;
    try {
      const value = await fetch();
      if (ticket === newest) apply(value);
    } catch (error) {
      if (ticket === newest) fail(error);
    } finally {
      inFlight--;
    }
  }
  /** A poll tick: nothing while a load is in flight. */
  function tick() {
    return inFlight > 0 ? Promise.resolve() : load();
  }
  return { load, tick };
}
