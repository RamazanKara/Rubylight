// Game settings files swapped per device (the host's butterpollo_core::game_settings): whole
// files, each a path with a drive letter or a %NAME% first.

export const SETTINGS_FILES_KEY = 'game-settings-files';
export const MAX_SETTINGS_FILES = 16;

/** The paths listed, trimmed, blanks dropped. */
export function settingsFilePaths(values: string[]): string[] {
  return values.map((value) => value.trim()).filter(Boolean);
}

/** The paths an app record lists. */
export function storedSettingsFiles(value: unknown): string[] {
  return Array.isArray(value) ? value.filter((entry): entry is string => typeof entry === 'string') : [];
}

/** What is wrong with one path, or '': the host's own checks, so a save is not refused. */
export function settingsFileError(path: string): string {
  const value = path.trim();
  if (!value) return '';
  if (/[\u0000-\u001f]/.test(value) || value.length > 1024) return 'A path is one line of at most 1024 characters.';
  if ((value.split('%').length - 1) % 2) return 'Write environment variables as %NAME%.';
  if (!/^([A-Za-z]:[\\/]|%[A-Za-z0-9_()-]+%)/.test(value)) {
    return 'Start with a drive letter, such as C:\\, or a variable such as %LOCALAPPDATA%.';
  }
  if (value.slice(2).replace(/%[^%]*%/g, 'x').includes(':')) return 'A colon belongs only after the drive letter.';
  if (value.split(/[\\/]/).some((part) => part.trim() === '..')) return 'Name the file’s own folder, without “..”.';
  if (/[\\/]$/.test(value)) return 'Name the settings file, not its folder.';
  return '';
}

/** Why the list cannot be saved, or ''. */
export function settingsFilesError(values: string[]): string {
  const paths = settingsFilePaths(values);
  const wrong = paths.map(settingsFileError).find(Boolean);
  if (wrong) return wrong;
  if (paths.length > MAX_SETTINGS_FILES) return `At most ${MAX_SETTINGS_FILES} files.`;
  const seen = new Set<string>();
  for (const path of paths) {
    const normal = path.replaceAll('/', '\\').toLowerCase();
    if (seen.has(normal)) return `${path} is listed twice.`;
    seen.add(normal);
  }
  return '';
}
