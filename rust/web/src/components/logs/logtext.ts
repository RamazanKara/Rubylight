// Parsing and search for the log viewer. The host writes tracing records:
// `2026-10-03T16:36:10.638909Z  INFO butterpollo::stream: CLIENT CONNECTED client=samsung`

export type Level = 'TRACE' | 'DEBUG' | 'INFO' | 'WARN' | 'ERROR';
export type LevelFilter = 'all' | 'warn' | 'error';

export interface LogLine {
  id: number;
  /** The line without colour codes. */
  text: string;
  /** Lines without a record header (backtraces, multi-line values) take the level of the record above. */
  level: Level | null;
  /** Length of the leading timestamp; 0 when the line has none. */
  time: number;
}

const ANSI = /\x1b\[[0-9;]*[A-Za-z]/g;
const RECORD = /^(\d{4}-\d\d-\d\dT\S+)\s+(TRACE|DEBUG|INFO|WARN|ERROR)\b/;

let nextId = 1;

/** Parse complete lines; `previous` is the level of the line before the first one. */
export function parse(raw: string[], previous: Level | null): LogLine[] {
  let level = previous;
  return raw.map((line) => {
    const text = line.replace(ANSI, '').replace(/\r$/, '');
    const record = RECORD.exec(text);
    if (record) level = record[2] as Level;
    return { id: nextId++, text, level, time: record?.[1]?.length ?? 0 };
  });
}

export function passes(line: LogLine, filter: LevelFilter): boolean {
  if (filter === 'error') return line.level === 'ERROR';
  if (filter === 'warn') return line.level === 'WARN' || line.level === 'ERROR';
  return true;
}

/** Case-insensitive literal search; null when there is nothing to look for. */
export function searchPattern(query: string): RegExp | null {
  if (!query.trim()) return null;
  return new RegExp(query.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'), 'gi');
}

export function countMatches(text: string, pattern: RegExp): number {
  return text.match(pattern)?.length ?? 0;
}

/** A run of text in a line: inside the timestamp or not, and the match it belongs to (-1 for none). */
export interface Piece {
  text: string;
  time: boolean;
  hit: number;
}

/** Matches marked in one line: a line of thousands would otherwise become thousands of elements. */
export const MAX_MARKS = 400;

/** The line cut at its matches. Past MAX_MARKS matches, only those around match `around` are cut out. */
export function pieces(line: LogLine, pattern: RegExp, around = 0): Piece[] {
  const first = Math.max(0, around - MAX_MARKS / 2);
  const out: Piece[] = [];
  const push = (from: number, to: number, hit: number) => {
    if (from < line.time && to > line.time) {
      out.push({ text: line.text.slice(from, line.time), time: true, hit });
      out.push({ text: line.text.slice(line.time, to), time: false, hit });
    } else if (to > from) {
      out.push({ text: line.text.slice(from, to), time: from < line.time, hit });
    }
  };
  let at = 0;
  let hit = 0;
  for (const match of line.text.matchAll(pattern)) {
    const n = hit++;
    if (n < first || n >= first + MAX_MARKS) continue;
    push(at, match.index, -1);
    at = match.index + match[0].length;
    push(match.index, at, n);
  }
  push(at, line.text.length, -1);
  return out;
}

/** Index of the line with `id`, or of the nearest line after it (the last one when none is); ids ascend. */
export function indexOfId(lines: LogLine[], id: number): number {
  let low = 0;
  let high = lines.length - 1;
  while (low < high) {
    const middle = (low + high) >> 1;
    if (lines[middle]!.id < id) low = middle + 1;
    else high = middle;
  }
  return Math.max(0, low);
}

/**
 * The ids of the first and last line whose matches are marked: `span` lines either side of line `anchor`.
 * Marking every match of a short search in 5000 lines made tens of thousands of elements and froze the page.
 */
export function markedRange(lines: LogLine[], anchor: number, span: number): { from: number; to: number } {
  if (!lines.length) return { from: 0, to: -1 };
  const at = indexOfId(lines, anchor);
  return { from: lines[Math.max(0, at - span)]!.id, to: lines[Math.min(lines.length - 1, at + span)]!.id };
}
