// The library's select mode and the removed games a restore can bring back.
import type { App, DiscoveredGame, LibrarySource } from '../../lib/api';

export type Launcher = LibrarySource | 'playnite';

/** The launchers' names, as the host's launcher list shows them. */
export const LAUNCHER_NAMES: Record<Launcher, string> = {
  steam: 'Steam',
  epic: 'Epic Games',
  battlenet: 'Battle.net',
  ea: 'EA app',
  ubisoft: 'Ubisoft Connect',
  gog: 'GOG',
  microsoft: 'Microsoft Store and Xbox',
  local: 'Game folders',
  playnite: 'Playnite',
};
const isLauncher = (key: unknown): key is Launcher => typeof key === 'string' && Object.hasOwn(LAUNCHER_NAMES, key);

/** The launcher an app's game comes from, by the marks the syncs and imports leave; null for the user's own programs. */
export function launcherOf(app: App): Launcher | null {
  if (isLauncher(app['launcher-source'])) return app['launcher-source'];
  if (typeof app['playnite-id'] === 'string' && app['playnite-id']) return 'playnite';
  if (app['steam-id'] !== undefined && app['steam-id'] !== '') return 'steam';
  // The host's Steam uuids: steam-<appid>, or its canonical STEAM uuid.
  if (typeof app.uuid === 'string' && /^(steam-\d|53544541-4d00-5000-8000-[0-9a-f]{12}$)/i.test(app.uuid)) return 'steam';
  if (typeof app.cmd === 'string' && /steam:\/\/(rungameid|run)\/\d/i.test(app.cmd)) return 'steam';
  return null;
}

/**
 * The editor's Delete wording: a launcher's game goes onto the host's removed list
 * (`library::dismissal_keys`), so Restore removed games brings it back; anything else is gone.
 */
export function deleteMessage(app: App): string {
  return launcherOf(app)
    ? 'It is removed from the library and devices can no longer start it. No sync adds it back; Restore removed games under Where your games come from brings it back.'
    : 'It is removed from the library and devices can no longer start it. This can’t be undone.';
}

/** The launchers among the apps with how many apps each has, in name order. */
export function launcherCounts(apps: App[]): { launcher: Launcher; name: string; count: number }[] {
  const counts = new Map<Launcher, number>();
  for (const app of apps) {
    const launcher = launcherOf(app);
    if (launcher) counts.set(launcher, (counts.get(launcher) ?? 0) + 1);
  }
  return [...counts]
    .map(([launcher, count]) => ({ launcher, name: LAUNCHER_NAMES[launcher], count }))
    .sort((a, b) => a.name.localeCompare(b.name));
}

/** The selection with these apps added; apps without a uuid can't be removed and are left out. */
export function selectApps(selected: ReadonlySet<string>, apps: App[]): Set<string> {
  const next = new Set(selected);
  for (const app of apps) if (app.uuid) next.add(app.uuid);
  return next;
}

/** The selection with one app (or removed game) switched. */
export function toggled(selected: ReadonlySet<string>, key: string): Set<string> {
  const next = new Set(selected);
  if (!next.delete(key)) next.add(key);
  return next;
}

/** "1 app" or "3 apps". */
export const counted = (count: number, word: string) => `${count} ${word}${count === 1 ? '' : 's'}`;

export interface RemovedGame {
  /** The `launcher-dismissed` key the host takes back. */
  key: string;
  launcher: string;
  name: string;
}

/**
 * The removed games for the restore list, named from the launchers' installed games. A game
 * the launchers no longer have, or a Playnite game, is named by its launcher.
 */
export function removedGames(keys: string[], installed: DiscoveredGame[]): RemovedGame[] {
  const names = new Map(installed.map((game) => [`${game.source}:${game.id}`, game.name]));
  return keys
    .map((key) => {
      const source = key.slice(0, key.indexOf(':'));
      const launcher = isLauncher(source) ? LAUNCHER_NAMES[source] : source;
      const name = names.get(key) || (source === 'playnite' ? 'A Playnite game' : `A game no longer installed (${key.slice(source.length + 1)})`);
      return { key, launcher, name };
    })
    .sort((a, b) => a.launcher.localeCompare(b.launcher) || a.name.localeCompare(b.name, undefined, { numeric: true, sensitivity: 'base' }));
}
