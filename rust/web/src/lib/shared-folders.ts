// The shared_folders setting and each device's grants for its folders.
//
// The host reads shared_folders as a JSON list of {id, name, path}
// (config.rs, parse_shared_folders) and shares nothing while any entry is
// refused, so the console checks every rule the host does before saving.

export interface SharedFolder {
  /** What device grants name. Empty for a folder not saved yet: saving gives it one from its name and `fresh`. */
  id: string;
  name: string;
  path: string;
  /** Random letters a folder added here gets, so its id never matches a grant left by a removed folder. Not stored. */
  fresh?: string;
}

export type FolderAccess = 'read' | 'write';
export type FolderGrants = Record<string, FolderAccess>;

/** The folders the value lists, and why it could not be read when it is not a list of folders. */
export interface FolderList {
  folders: SharedFolder[];
  problem: string;
}

const text = (value: unknown) => (typeof value === 'string' ? value : value === undefined || value === null ? '' : String(value));

/** The setting as GET /api/config gives it: a parsed list, its text, or nothing. */
export function readFolders(value: unknown): FolderList {
  let list = value;
  if (list === null || list === undefined) return { folders: [], problem: '' };
  if (typeof list === 'string') {
    if (list.trim() === '') return { folders: [], problem: '' };
    try {
      list = JSON.parse(list);
    } catch (error) {
      return { folders: [], problem: `it is not JSON (${error instanceof Error ? error.message : String(error)})` };
    }
  }
  if (!Array.isArray(list)) return { folders: [], problem: 'it is not a list' };
  const folders: SharedFolder[] = [];
  for (const entry of list as unknown[]) {
    if (!entry || typeof entry !== 'object' || Array.isArray(entry)) return { folders: [], problem: 'an entry is not a folder' };
    const { id, name, path, fresh } = entry as Record<string, unknown>;
    const folder: SharedFolder = { id: text(id), name: text(name), path: text(path) };
    if (!folder.id && typeof fresh === 'string') folder.fresh = fresh;
    folders.push(folder);
  }
  return { folders, problem: '' };
}

/** config.rs, valid_folder_id: 1 to 64 ASCII letters, digits, - or _. */
export function validFolderId(id: string): boolean {
  return /^[A-Za-z0-9_-]{1,64}$/.test(id);
}

/**
 * config.rs, valid_folder_root: a folder on a drive of this PC, such as
 * D:\Games. A network share, a \\?\ or \\.\ path, a relative path, . or ..,
 * and a colon past the drive (a stream name) are refused.
 */
export function validFolderRoot(path: string): boolean {
  return /^[A-Za-z]:[\\/]/.test(path) && !path.slice(2).includes(':') && !path.slice(3).split(/[\\/]/).some((part) => part === '.' || part === '..');
}

const blank = (folder: SharedFolder) => folder.id === '' && folder.name.trim() === '' && folder.path.trim() === '';

/** What is wrong with a folder's name, path and saved id; an empty string where nothing is. */
export interface FolderProblems {
  name: string;
  path: string;
  id: string;
}

/** The problems of each folder, by index. A row left blank is dropped on save, so it has none. */
export function folderErrors(folders: readonly SharedFolder[]): FolderProblems[] {
  const names = new Map<string, number>();
  const ids = new Map<string, number>();
  for (const folder of folders) {
    const name = folder.name.trim().toLowerCase();
    if (name) names.set(name, (names.get(name) ?? 0) + 1);
    if (folder.id) ids.set(folder.id, (ids.get(folder.id) ?? 0) + 1);
  }
  return folders.map((folder) => {
    if (blank(folder)) return { name: '', path: '', id: '' };
    const name = folder.name.trim();
    const path = folder.path.trim();
    return {
      name: !name ? 'Enter a name.' : (names.get(name.toLowerCase()) ?? 0) > 1 ? 'Another folder has this name.' : '',
      path: !path
        ? 'Enter the folder’s full path, such as D:\\Games.'
        : !validFolderRoot(path)
          ? 'Enter a folder on a drive of this PC, such as D:\\Games. Network shares, relative paths, . and .. are not shared.'
          : '',
      id: !folder.id
        ? ''
        : !validFolderId(folder.id)
          ? `The host refuses the saved id “${folder.id}”. Remove this folder and add it again.`
          : (ids.get(folder.id) ?? 0) > 1
            ? `Another folder has the saved id “${folder.id}”. Remove one and add it again.`
            : '',
    };
  });
}

export function hasFolderErrors(folders: readonly SharedFolder[]): boolean {
  return folderErrors(folders).some((errors) => errors.name || errors.path || errors.id);
}

/** A folder added in the console, with its random letters. */
export function newFolder(): SharedFolder {
  const bytes = crypto.getRandomValues(new Uint8Array(4));
  return { id: '', name: '', path: '', fresh: Array.from(bytes, (byte) => 'abcdefghijklmnopqrstuvwxyz234567'[byte % 32]).join('') };
}

/**
 * An id made from the name (lowercase letters and digits joined by -, or
 * "folder") and the folder's random letters, such as games-k2fq.
 */
export function folderId(name: string, fresh: string, taken: ReadonlySet<string>): string {
  const slug =
    name
      .normalize('NFKD')
      .replace(/[\u0300-\u036f]/g, '')
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, '-')
      .replace(/^-+|-+$/g, '')
      .slice(0, 40)
      .replace(/-+$/, '') || 'folder';
  const tag = fresh.replace(/[^a-z0-9]/g, '').slice(0, 16);
  const base = tag ? `${slug}-${tag}` : slug;
  let id = base;
  for (let n = 2; taken.has(id); n++) id = `${base}-${n}`;
  return id;
}

/**
 * The folders as the host stores them: trimmed, blank rows dropped, and an
 * id for each folder not saved yet. A saved folder keeps its id, so renaming
 * it keeps the devices' grants; one added later never takes over the grants
 * of one removed, even with the same name.
 */
export function storedFolders(folders: readonly SharedFolder[]): SharedFolder[] {
  const kept = folders.filter((folder) => !blank(folder));
  const taken = new Set(kept.map((folder) => folder.id).filter(Boolean));
  return kept.map((folder) => {
    const name = folder.name.trim();
    let id = folder.id;
    if (!id) {
      id = folderId(name, folder.fresh ?? '', taken);
      taken.add(id);
    }
    return { id, name, path: folder.path.trim() };
  });
}

/** A device's grants as the host keeps them; anything but read or write grants nothing there and is left out. */
export function readGrants(value: unknown): FolderGrants {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return {};
  const grants: FolderGrants = {};
  for (const [id, access] of Object.entries(value as Record<string, unknown>)) {
    if (access === 'read' || access === 'write') grants[id] = access;
  }
  return grants;
}

/** The grants as /api/clients/update takes them, keys sorted so equal grants compare equal; null clears them. */
export function grantsValue(grants: FolderGrants): FolderGrants | null {
  const entries = Object.entries(grants).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0));
  return entries.length ? Object.fromEntries(entries) : null;
}

/** The grants with `id` set to `access`, or removed for none. */
export function withGrant(grants: FolderGrants, id: string, access: FolderAccess | ''): FolderGrants {
  const next = { ...grants };
  if (access) next[id] = access;
  else delete next[id];
  return next;
}

export interface GrantRow {
  id: string;
  name: string;
  path: string;
  access: FolderAccess | '';
  /** A grant for a folder the PC no longer shares. It grants nothing until a folder has that id again. */
  gone: boolean;
}

/** One row for each shared folder, then one for each grant naming a folder no longer shared. */
export function grantRows(folders: readonly SharedFolder[], grants: FolderGrants): GrantRow[] {
  const rows: GrantRow[] = folders
    .filter((folder) => validFolderId(folder.id))
    .map((folder) => ({ id: folder.id, name: folder.name, path: folder.path, access: grants[folder.id] ?? '', gone: false }));
  for (const [id, access] of Object.entries(grants)) {
    if (!rows.some((row) => row.id === id)) rows.push({ id, name: id, path: '', access, gone: true });
  }
  return rows;
}
