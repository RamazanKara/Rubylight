import type { CoopGame, CoopStatus } from './api';

/**
 * What the game page's Play together panel shows for one app, from `GET /api/coop`. Pure, so every
 * state is checked from fixtures (CoopView.test.mjs) without a host.
 *
 * The three states of the brief: `setup` (co-op isn't set up), `handler` (no handler for this game, or
 * the game not added in Nucleus Co-op yet) and `ready`. Around them: `hidden` (the app starts nothing,
 * or the host is older), `unavailable` (this PC can't run co-op: setting it up would download for
 * nothing), `launcher` (a launcher's link starts the app, so no handler can match it), `installing`
 * and `failed` (the download), and `broken` (the handlers didn't load).
 */
export type PlayTogetherView =
  | { kind: 'hidden' }
  | { kind: 'unavailable'; reason: string }
  | { kind: 'launcher'; text: string }
  | { kind: 'setup'; text: string; download: string }
  | { kind: 'installing'; label: string; percent: number | null }
  | { kind: 'failed'; error: string }
  | { kind: 'broken'; error: string }
  | { kind: 'checking' }
  | { kind: 'handler'; added: boolean; text: string; steps: string }
  | { kind: 'ready'; players: string; entry: string; start: string };

/** Megabytes as the release page counts them, kept on one line. */
export function megabytes(bytes: number): string {
  return `${Math.round(bytes / 1_000_000)}\u00a0MB`;
}

/** The host's reasons are clauses; the panel shows them as sentences. */
function sentence(text: string): string {
  return /[.!?…]$/.test(text) ? text : `${text}.`;
}

/** The download the Set up button starts, named before it runs. */
export function downloadText(status: CoopStatus): string {
  const release = status.install.release;
  if (!release) return 'Downloads Nucleus Co-op from its GitHub release.';
  return `Downloads Nucleus Co-op ${release.version} (${megabytes(release.size)}) from its GitHub release.`;
}

/** The download's progress line and bar; the last step checks the archive and unpacks it. */
export function installProgress(status: CoopStatus): { label: string; percent: number | null } {
  const size = status.install.release?.size ?? 0;
  const received = status.install.received ?? 0;
  if (!size) return { label: 'Downloading Nucleus Co-op…', percent: null };
  if (received >= size) return { label: 'Checking the download and unpacking it…', percent: 100 };
  return {
    label: `Downloading Nucleus Co-op… ${megabytes(received)} of ${megabytes(size)}`,
    percent: Math.min(99, Math.floor((100 * received) / size)),
  };
}

export function gameOf(status: CoopStatus, app: string): CoopGame | undefined {
  return status.games?.find((game) => game.app === app);
}

export function playTogether(status: CoopStatus, app: string): PlayTogetherView {
  const game = gameOf(status, app);
  // An older host lists no games; an app without an .exe has no co-op.
  if (!game || game.status === 'no_executable') return { kind: 'hidden' };
  if (!status.available) {
    return { kind: 'unavailable', reason: sentence(status.reason ?? 'Nucleus Co-op can’t run on this PC') };
  }
  if (game.status === 'launcher') {
    return {
      kind: 'launcher',
      text: `Co-op finds a game by its .exe, and ${game.name} starts through a launcher. To play it together, set the command above to the game’s .exe.`,
    };
  }
  switch (status.install.state) {
    case 'missing':
      return {
        kind: 'setup',
        text: 'Play this game together, or play it while someone uses this PC.',
        download: downloadText(status),
      };
    case 'installing':
      return { kind: 'installing', ...installProgress(status) };
    case 'failed':
      return { kind: 'failed', error: status.install.error ?? 'Nucleus Co-op could not be set up.' };
  }
  if (status.catalog_error) return { kind: 'broken', error: status.catalog_error };
  const name = game.name;
  switch (game.status) {
    case 'ready': {
      const players = game.players ?? 1;
      return {
        kind: 'ready',
        players: `Up to ${players} ${players === 1 ? 'player' : 'players'}.`,
        entry: game.entry,
        start: `Start “${game.entry}” from Nightjar or Moonlight. Each device that starts it next joins as the next player.`,
      };
    }
    case 'no_handler':
      return {
        kind: 'handler',
        added: false,
        text: `Nucleus Co-op needs a handler for ${name}.`,
        steps: `Nucleus Co-op opens on the PC’s screen. There, search its handler hub for ${name}, download the handler, then add the game. This panel moves on by itself.`,
      };
    case 'not_added':
      return {
        kind: 'handler',
        added: true,
        text: `Nucleus Co-op has a handler for ${name}, but the game isn’t added yet.`,
        steps: `Nucleus Co-op opens on the PC’s screen. There, add ${name} once, choosing the .exe this app starts. This panel moves on by itself.`,
      };
    default:
      return { kind: 'checking' };
  }
}

/** The panel polls while something changes on its own: the download, or a handler being added on the PC. */
export function pollsView(view: PlayTogetherView): boolean {
  return view.kind === 'installing' || view.kind === 'handler' || view.kind === 'checking';
}

/** The limits, said where co-op is ready. */
export const COOP_LIMITS = 'Controllers only: keyboard and mouse don’t reach the game. Paired devices can join unless Join co-op is off on their page under Devices.';

/** For a console opened away from the PC: what happens where. */
export const AWAY_FROM_PC = 'Nucleus Co-op opens on the PC’s own screen, not on this device. Do the steps at the PC; this page shows the game as ready once the handler is in.';
