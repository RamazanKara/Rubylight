import { PERM, type Client, type CoopGame, type CoopSession, type CoopStatus } from './api';

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
export function sentence(text: string): string {
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
export const COOP_LIMITS = 'Controllers only: keyboard and mouse don’t reach the game. A paired device that can play games joins unless “Can join co-op games” is off on its page under Devices.';

// ---------- Home: one card per live co-op game ----------

/** "Can join co-op games" as stored: true, false, or null when the device never set it. */
export function storedCoopJoin(value: unknown): boolean | null {
  if (value === undefined || value === null) return null;
  // As the host reads it (coop::may_join): true and "true" join, anything else set keeps the device out.
  return value === true || value === 'true';
}

/**
 * Whether a device may join co-op games, as the host decides: joining is a launch of the co-op entry,
 * so it needs Launch apps; never set, it follows Launch apps, and set to false it keeps the device out.
 */
export function joinsCoop(perm: number, stored: unknown): boolean {
  return (perm & PERM.LAUNCH_APPS) !== 0 && (storedCoopJoin(stored) ?? true);
}

/** The library entry a co-op game starts from (the host's `nucleus::ENTRY_SUFFIX`). */
export function entryName(title: string): string {
  return `${title} (Co-op)`;
}

/** The co-op games running now, one Home card each. A background stream shows as a stream, not here. */
export function liveCoop(status: CoopStatus | null): CoopSession[] {
  return (status?.sessions ?? []).filter((session) => session.kind === 'coop' && session.phase !== 'ended');
}

/** Minutes and seconds, as a countdown reads: 1:05, 0:09. */
export function clock(seconds: number): string {
  const whole = Math.max(0, Math.ceil(seconds));
  return `${Math.floor(whole / 60)}:${String(whole % 60).padStart(2, '0')}`;
}

export interface Seat {
  slot: number;
  /** "Player 1": the number Nucleus Co-op and the controller show. */
  label: string;
  /** The device's name; null for a free slot. */
  name: string | null;
  device: string | null;
  /** While the device's stream is gone: how long it keeps the slot. */
  away: string | null;
  /** The same without "Away, ", beside the card's Away badge: "keeps the slot for 1:24". */
  keeps: string | null;
}

/**
 * Each slot of a session, seated or free. An away device keeps its slot for the host's 120 s grace;
 * `elapsed` seconds since the session was read count it down between polls.
 */
export function seats(session: CoopSession, elapsed = 0): Seat[] {
  return session.players.map((player) => {
    const keeps = !player.device || !player.away
      ? null
      : typeof player.rejoin_in === 'number'
        ? `keeps the slot for ${clock(player.rejoin_in - elapsed)}`
        : 'keeps the slot for 2 minutes';
    return {
      slot: player.slot,
      label: `Player ${player.slot + 1}`,
      name: player.device ? (player.name ?? 'Unnamed device') : null,
      device: player.device,
      away: keeps && `Away, ${keeps}`,
      keeps,
    };
  });
}

/** "2 of 4 players · 2 free slots". */
export function headcount(session: CoopSession): string {
  const seated = session.players.filter((player) => player.device).length;
  const free = session.players.length - seated;
  const slots = free === 0 ? 'no free slot' : free === 1 ? '1 free slot' : `${free} free slots`;
  return `${seated} of ${session.players.length} ${session.players.length === 1 ? 'player' : 'players'} · ${slots}`;
}

export interface Invitee {
  uuid: string;
  name: string;
  /** Already allowed: it only has to start the game. */
  canJoin: boolean;
}

/**
 * Paired devices that could take a free slot: on, allowed to launch apps (a watch-only device can't
 * start the co-op entry), and not seated in this game (an away device keeps its slot and comes back
 * by itself). Empty while the game has no free slot.
 */
export function invitees(session: CoopSession, clients: Client[]): Invitee[] {
  if (!session.players.some((player) => !player.device)) return [];
  const seated = new Set(session.players.flatMap((player) => (player.device ? [player.device] : [])));
  return clients
    .filter((client) => client.enabled !== false && ((client.perm ?? 0) & PERM.LAUNCH_APPS) !== 0 && !seated.has(client.uuid))
    .map((client) => ({ uuid: client.uuid, name: client.name, canJoin: joinsCoop(client.perm ?? 0, client.coop_join) }));
}

/** What a device does to join once it may. */
export function joinSteps(session: CoopSession): string {
  return `Open Nightjar or Moonlight on it and pick “${entryName(session.title)}”.`;
}

export interface Confirmation {
  title: string;
  message: string;
  confirm: string;
  danger: boolean;
}

export function removeConfirmation(seat: Seat): Confirmation {
  const name = seat.name ?? seat.label;
  return { title: `Remove ${name}?`, message: `${name} leaves the game and can’t rejoin this session.`, confirm: 'Remove', danger: true };
}

export function endConfirmation(session: CoopSession): Confirmation {
  return {
    title: `End co-op in ${session.title}?`,
    message: `Ends every player’s stream and closes ${session.title} on this PC.`,
    confirm: 'End co-op',
    danger: true,
  };
}

/** Beside Close Nucleus Co-op once the game is ready: the window the console opened is still on the PC's screen. */
export const OPEN_ON_PC = 'Nucleus Co-op is still open on the PC’s screen. Co-op closes it when a game starts.';

/** For a console opened away from the PC: what happens where. */
export const AWAY_FROM_PC = 'Nucleus Co-op opens on the PC’s own screen, not on this device. Do the steps at the PC; this page shows the game as ready once the handler is in.';
