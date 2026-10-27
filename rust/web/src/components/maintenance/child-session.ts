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
 * child session, `inside` in the child session's own host, `setup` while
 * Windows settings are missing (or could not be read), `ready` to start.
 */
export function step(status: ChildSessionStatus): 'off' | 'inside' | 'setup' | 'ready' {
  if (status.mode === 'inside_child_session') return 'inside';
  if (status.mode === 'virtual_display') return 'off';
  return status.missing?.length === 0 ? 'ready' : 'setup';
}
