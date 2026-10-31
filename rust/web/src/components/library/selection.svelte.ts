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

/** What to tell the user after a restore, by whether the sync after it ran. */
export function restoreMessage(restored: number, synced: boolean, automatic: boolean): { text: string; tone: 'ok' | 'warn' } {
  const games = counted(restored, 'game');
  if (!automatic) return { text: `Restored ${games}: pick them again in Add games.`, tone: 'ok' };
  if (!synced) return { text: `Restored ${games}: they join your library with the next sync.`, tone: 'warn' };
  return { text: `Restored ${games}: the installed ones are back in your library.`, tone: 'ok' };
}

/** The host's answer to a bulk remove: apps kept because Neural Rendering stayed in their folder. */
export interface Removal {
  deleted: number;
  kept?: { name: string; error: string }[];
}

/** What to tell the user after removing `total` apps. */
export function removalMessage(total: number, what: string, removal: Removal): { text: string; tone: 'ok' | 'warn' } {
  const kept = removal.kept ?? [];
  if (kept.length > 0) {
    const names = kept.map((app) => app.name).join(', ');
    return {
      text: `Removed ${removal.deleted} of ${counted(total, 'app')}. Kept ${names}: ${kept[0]?.error ?? ''}`,
      tone: 'warn',
    };
  }
  if (removal.deleted < total) {
    return { text: `Removed ${removal.deleted} of ${counted(total, 'app')}; the others were already gone.`, tone: 'ok' };
  }
  return { text: `Removed ${what}.`, tone: 'ok' };
}

/** The library grid's select mode: which apps are picked, and their removal. */
export class AppSelection {
  selecting = $state(false);
  selected = $state.raw<Set<string>>(new Set());

  /** The picked apps still in the list: one gone (a sync, another tab) drops out. */
  chosen(apps: App[]): App[] {
    return apps.filter((app) => app.uuid && this.selected.has(app.uuid));
  }
  start() {
    this.selecting = true;
  }
  stop() {
    this.selecting = false;
    this.selected = new Set();
  }
  toggle(uuid: string) {
    this.selected = toggled(this.selected, uuid);
  }
  add(apps: App[]) {
    this.selected = selectApps(this.selected, apps);
  }
  addLauncher(apps: App[], launcher: Launcher) {
    this.add(apps.filter((app) => launcherOf(app) === launcher));
  }
  clear() {
    this.selected = new Set();
  }

  /**
   * Removes the picked apps in one request and leaves select mode. A failed request throws
   * and keeps the selection for another try.
   */
  async remove(apps: App[], removeMany: (uuids: string[]) => Promise<Removal>) {
    const list = this.chosen(apps);
    const what = list.length === 1 ? list[0]?.name || 'this app' : counted(list.length, 'app');
    const removal = await removeMany(list.flatMap((app) => (app.uuid ? [app.uuid] : [])));
    this.stop();
    return removalMessage(list.length, what, removal);
  }
}

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
