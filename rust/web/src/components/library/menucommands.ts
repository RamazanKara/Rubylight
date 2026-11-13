// An app's own stream-menu commands (`menu-cmd`): devices with the PC commands
// permission see them after the host-wide PC commands while the app runs. The
// undo runs when the app closes, once for each command that ran.

export const MENU_KEY = 'menu-cmd';
/** The host refuses longer lists. */
export const MENU_LIMIT = 64;

export interface MenuCommand {
  /** Foundation Sunshine's command id, kept when an imported app has one. */
  id?: string;
  name: string;
  cmd: string;
  undo?: string;
  elevated?: boolean;
}

export interface MenuRow {
  key: number;
  id?: string;
  name: string;
  cmd: string;
  undo: string;
  elevated: boolean;
}

let nextKey = 1;
export const menuRow = (command: Partial<MenuCommand> = {}): MenuRow => ({
  key: nextKey++,
  ...(typeof command.id === 'string' ? { id: command.id } : {}),
  name: typeof command.name === 'string' ? command.name : '',
  cmd: typeof command.cmd === 'string' ? command.cmd : '',
  undo: typeof command.undo === 'string' ? command.undo : '',
  elevated: command.elevated === true,
});

export function menuRows(value: unknown): MenuRow[] {
  if (!Array.isArray(value)) return [];
  return value.flatMap((entry: unknown) =>
    entry && typeof entry === 'object' ? [menuRow(entry as Partial<MenuCommand>)] : [],
  );
}

const blank = (row: MenuRow) => !row.name.trim() && !row.cmd.trim() && !row.undo.trim();

/** The list to store: blank rows are dropped, an empty undo is left out. */
export function menuEntries(rows: MenuRow[]): MenuCommand[] {
  return rows
    .filter((row) => !blank(row))
    .map((row) => ({
      ...(row.id ? { id: row.id } : {}),
      name: row.name.trim(),
      cmd: row.cmd,
      ...(row.undo.trim() ? { undo: row.undo } : {}),
      elevated: row.elevated,
    }));
}

/** Why the list cannot be saved, or ''. */
export function menuError(rows: MenuRow[]): string {
  const used = rows.filter((row) => !blank(row));
  if (used.length > MENU_LIMIT) return `An app can have at most ${MENU_LIMIT} menu commands.`;
  if (used.some((row) => !row.name.trim())) return 'Give every menu command a name.';
  if (used.some((row) => !row.cmd.trim())) return 'Give every menu command a command line.';
  return '';
}
