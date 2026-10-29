// Managed game files: keys in a game's own .ini that the host sets for a stream and puts back
// when the session ends, as `game-files` in the app or in a device's block. The host checks
// them when the app is saved and again at launch (rust/core/src/game_files.rs); the checks here
// only spare a round trip.

export const GAME_FILES_KEY = 'game-files';
/** The most keys one list holds. */
export const MAX_FILES = 32;
const MAX_TEXT = 256;
const MAX_PATH = 1024;

export interface GameFileEntry {
  path: string;
  section: string;
  key: string;
  value: string | number | boolean;
}
export interface GameFileRow {
  /** Keeps each row's inputs in place when rows move. */
  id: number;
  path: string;
  section: string;
  key: string;
  /** As typed: text, a number or {stream_fps} with an offset. */
  value: string;
  /** Fields of the stored entry this editor does not show, kept as they are. */
  rest: Record<string, unknown>;
}

let nextId = 1;
export const gameFileRow = (entry: Record<string, unknown> = {}): GameFileRow => {
  const { path, section, key, value, ...rest } = entry;
  return {
    id: nextId++,
    path: typeof path === 'string' ? path : '',
    section: typeof section === 'string' ? section : '',
    key: typeof key === 'string' ? key : '',
    value: ['string', 'number', 'boolean'].includes(typeof value) ? String(value) : '',
    rest,
  };
};

/** The rows for a stored list; anything that is not a list gives none. */
export function gameFileRows(list: unknown): GameFileRow[] {
  if (!Array.isArray(list)) return [];
  return list.flatMap((entry: unknown) =>
    entry && typeof entry === 'object' && !Array.isArray(entry) ? [gameFileRow(entry as Record<string, unknown>)] : [],
  );
}

const blank = (row: GameFileRow) => !row.path.trim() && !row.section.trim() && !row.key.trim() && !row.value.trim();

/** A whole number without leading zeros is stored as a number; anything else as typed. */
function valueOf(text: string): string | number {
  const trimmed = text.trim();
  return /^-?(0|[1-9]\d{0,14})$/.test(trimmed) ? Number(trimmed) : trimmed;
}

/** The list to store: blank rows are dropped. */
export function gameFileEntries(rows: GameFileRow[]): GameFileEntry[] {
  return rows
    .filter((row) => !blank(row))
    .map((row) => ({
      ...row.rest,
      path: row.path.trim(),
      section: row.section.trim(),
      key: row.key.trim(),
      value: valueOf(row.value),
    }));
}

/** `{stream_fps}`, `{stream_fps+N}` or `{stream_fps-N}`: the value that follows the stream. */
const STREAM_FPS = /^\{\s*stream_fps\s*(?:[+-]\s*\d{1,4}\s*)?\}$/;

/** What is wrong with a value, or ''. */
export function valueError(text: string): string {
  const value = text.trim();
  if (!value) return 'Give the key a value.';
  if (value.includes('{') || value.includes('}')) {
    if (!STREAM_FPS.test(value)) return 'Use {stream_fps}, or {stream_fps-1} for one below the stream’s frame rate.';
    const offset = Number(value.replace(/[^\d]/g, '') || 0);
    return offset > 1000 ? 'The offset is at most 1000.' : '';
  }
  if (value.length > MAX_TEXT) return 'The value is too long.';
  return '';
}

/** What is wrong with a file as written, or ''. Mirrors the host's `check_path`. */
export function pathError(text: string): string {
  const path = text.trim();
  if (!path) return 'Name the file.';
  if (path.length > MAX_PATH) return 'The path is too long.';
  if (/^[\\/]/.test(path)) return 'Network and device paths are not taken. Name a file in the game’s folder.';
  const drive = /^[a-zA-Z]:[\\/]/.test(path);
  if (path.slice(drive ? 2 : 0).includes(':')) return 'A colon belongs only after the drive letter.';
  if (path.split(/[\\/]/).some((part) => part.trim() === '..')) return 'Name the file inside the game’s folder, without ..';
  if (!/\.ini$/i.test(path)) return 'Only .ini files are managed.';
  return '';
}

const same = (a: string, b: string) => a.trim().replace(/\//g, '\\').toLowerCase() === b.trim().replace(/\//g, '\\').toLowerCase();

/** What is wrong with a row, or ''. A blank row is dropped when saving and is not an error. */
export function gameFileError(row: GameFileRow, rows: GameFileRow[]): string {
  if (blank(row)) return '';
  const path = pathError(row.path);
  if (path) return path;
  const section = row.section.trim();
  if (section.startsWith('[') || section.includes(']')) return 'Write the section without [ and ].';
  if (section.length > MAX_TEXT) return 'The section is too long.';
  const key = row.key.trim();
  if (!key) return 'Name the key.';
  if (key.includes('=') || /^[[;#]/.test(key)) return 'The key is the name before the =.';
  if (key.length > MAX_TEXT) return 'The key is too long.';
  if (
    rows.some(
      (other) => other !== row && same(other.path, row.path) && same(other.section, row.section) && same(other.key, row.key),
    )
  )
    return 'Listed twice.';
  return valueError(row.value);
}

/** What stops a list from being saved, or ''. */
export function gameFilesError(rows: GameFileRow[]): string {
  const listed = rows.filter((row) => !blank(row));
  if (listed.length > MAX_FILES) return `List at most ${MAX_FILES} keys.`;
  return listed.map((row) => gameFileError(row, rows)).find(Boolean) ?? '';
}
