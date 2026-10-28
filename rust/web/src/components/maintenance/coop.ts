import type { CoopEnd, CoopPhase, CoopPlayer, CoopSession, CoopStatus } from '../../lib/api';

type Tone = 'neutral' | 'ok' | 'warn' | 'danger';

/** What the Nucleus Co-op install state means to the person at the console. */
export const installStates: Record<CoopStatus['install']['state'], { label: string; tone: Tone }> = {
  installed: { label: 'Installed', tone: 'ok' },
  configured: { label: 'Your own copy', tone: 'ok' },
  missing: { label: 'Not installed yet', tone: 'neutral' },
  installing: { label: 'Downloading', tone: 'neutral' },
  failed: { label: 'Install failed', tone: 'danger' },
};

export const coopPhases: Record<CoopPhase, { label: string; tone: Tone }> = {
  starting: { label: 'Starting', tone: 'neutral' },
  running: { label: 'Running', tone: 'ok' },
  ended: { label: 'Ended', tone: 'neutral' },
};

const endings: Record<CoopEnd['reason'], string> = {
  ended: 'Ended from the console or by the host',
  empty: 'Every player left and none came back in time',
  crashed: 'A game instance or Nucleus Co-op exited',
  failed: 'Nucleus Co-op did not start the game',
};

/** Why a session ended, with what stopped when the host says. A newer reason still shows. */
export function endText(end: CoopEnd): string {
  const why = endings[end.reason] ?? `Ended (${end.reason})`;
  return end.detail ? `${why}: ${end.detail}` : why;
}

/** A phase from a newer host shows as it is, never as a blank badge. */
export function phaseOf(phase: string): { label: string; tone: Tone } {
  return coopPhases[phase as CoopPhase] ?? { label: phase, tone: 'neutral' };
}

/** Player numbers start at 1, as Nucleus Co-op counts them; a free slot says so. */
export function playerText(player: CoopPlayer): string {
  const who = player.name ?? (player.device ? 'Unnamed device' : 'Free slot');
  return `Player ${player.slot + 1}: ${who}`;
}

/** Only a seated device can be removed. */
export function kickable(session: CoopSession, player: CoopPlayer): boolean {
  return session.phase !== 'ended' && player.device !== null;
}

/** The panel polls while something changes on its own: a session or a download. */
export function polls(status: CoopStatus): boolean {
  return status.install.state === 'installing' || status.sessions.some((session) => session.phase !== 'ended');
}
