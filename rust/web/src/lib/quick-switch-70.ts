// The Rubylight 7.0 command palette (Ctrl+K, QuickSwitcher.svelte): quick-switch.ts's search,
// grouped as Recent, Games, Devices, Pages, Host actions, Diagnostics and Settings. Host actions
// know the host's state: a theme the console doesn't use yet, Disconnect every stream while a
// device streams, and Quit game while a game runs. Pure, so QuickSwitch70.test.mjs checks it
// without a browser.
import { searchSettings, searchWords } from '../components/settings/search';
import type { App, Client } from './api';
import { advanced, primary } from './navigation';
import {
  ACTIONS, LIMITS, PAGES, deviceEntry, gameEntry, ranked, rankedSettings, score, settingEntry,
  type Entry, type Group,
} from './quick-switch';
import { categoriesOf, schema } from './settings-schema';
import type { Theme } from './theme';

export const RECENT_KEY = 'rubylight.palette.recent';
export const EMPTY_LIMITS = { recent: 4, games: 4, devices: 3 } as const;

/** What the palette knows about the console and the host when it opens. */
export interface PaletteContext {
  theme: Theme;
  /** Streams to devices right now. */
  streams: number;
  /** The running game's name when an app runs on this PC. */
  runningApp?: string | null;
  /** Recently chosen entry ids, most recent first. */
  recentIds?: readonly string[];
}

/** Pages that find and explain a problem: they have their own group. */
const DIAGNOSTIC_PATHS = ['/troubleshoot', '/sessions', '/logs'];
const DIAGNOSTIC_DETAIL: Record<string, string> = {
  '/troubleshoot': 'Checks, fixes and displays',
  '/sessions': 'Live figures and every past stream',
  '/logs': 'What this PC wrote down',
};

const THEMES: Record<Theme, Entry> = {
  light: { id: 'action:theme-light', label: 'Use the light theme', icon: 'sun', action: 'theme-light', keywords: 'appearance bright toggle switch colours mode' },
  dark: { id: 'action:theme-dark', label: 'Use the dark theme', icon: 'moon', action: 'theme-dark', keywords: 'appearance night toggle switch colours mode' },
  system: { id: 'action:theme-system', label: 'Follow the system theme', icon: 'monitor', action: 'theme-system', keywords: 'appearance automatic windows toggle switch colours mode' },
};

const STOP: Entry = { id: 'action:stop', label: 'Disconnect every stream', icon: 'stop', action: 'stop', keywords: 'stop streams end kick all devices' };
const QUIT: Entry = { id: 'action:quit', label: 'Quit game', icon: 'power', action: 'quit', keywords: 'quit close stop exit end running game app' };
const DISPLAYS: Entry = {
  id: 'action:displays', label: 'Restore displays', detail: 'Troubleshoot', icon: 'monitor', href: '/troubleshoot#displays',
  keywords: 'monitor layout virtual display screen black fix saved',
};
const BUNDLE = ACTIONS.find((entry) => entry.action === 'bundle')!;

/** Reads saved MRU entry ids from site storage. */
export function readRecent(storage: Pick<Storage, 'getItem'> | null = typeof localStorage === 'undefined' ? null : localStorage): string[] {
  if (!storage) return [];
  try {
    const parsed = JSON.parse(storage.getItem(RECENT_KEY) ?? '[]');
    return Array.isArray(parsed) ? parsed.filter((item): item is string => typeof item === 'string').slice(0, 6) : [];
  } catch {
    return [];
  }
}

/** Prepends `id` to MRU storage and returns the updated list. */
export function recordRecent(
  id: string,
  storage: Pick<Storage, 'getItem' | 'setItem'> | null = typeof localStorage === 'undefined' ? null : localStorage,
): string[] {
  const next = [id, ...readRecent(storage).filter((existing) => existing !== id)].slice(0, 6);
  try {
    storage?.setItem(RECENT_KEY, JSON.stringify(next));
  } catch {
    // Ignore blocked storage.
  }
  return next;
}

/** The host actions for this moment: what can't be done now is not offered. */
export function hostActions(context: PaletteContext): Entry[] {
  const themes = (['light', 'dark', 'system'] as Theme[]).filter((theme) => theme !== context.theme).map((theme) => THEMES[theme]);
  const running = context.runningApp?.trim();
  return [
    ...ACTIONS.filter((entry) => entry !== BUNDLE && entry.action !== 'restart'),
    ...(context.streams > 0 ? [{ ...STOP, detail: context.streams === 1 ? '1 stream' : `${context.streams} streams` }] : []),
    ...(running ? [{ ...QUIT, detail: running }] : []),
    DISPLAYS,
    ...themes,
    ...ACTIONS.filter((entry) => entry.action === 'restart'),
  ];
}

/** Troubleshoot, Sessions and Logs, and the support bundle. */
export function diagnostics(): Entry[] {
  const pages = DIAGNOSTIC_PATHS.flatMap((path) => {
    const item = advanced.find((candidate) => candidate.path === path);
    return item ? [{ id: `page:${item.path}`, label: item.label, detail: DIAGNOSTIC_DETAIL[path], keywords: item.keywords, icon: item.icon, href: item.path } satisfies Entry] : [];
  });
  return [...pages, BUNDLE];
}

/** How many host actions a query lists; with nothing typed, all of them show. */
export const PALETTE_LIMITS = { ...LIMITS, actions: 6, recent: EMPTY_LIMITS.recent } as const;

/**
 * The palette for a query. With nothing typed: MRU recent entries, top games and devices,
 * everyday pages, host actions (including Quit game while one runs) and diagnostics. With a query:
 * the group with the closest match first (on a tie pages, host actions, diagnostics, games, then
 * devices) and settings last, so Enter takes the closest match.
 */
export function palette(query: string, found: { apps: App[] | null; clients: Client[] | null }, context: PaletteContext): Group[] {
  const words = searchWords(query);
  const actions = hostActions(context);
  const checks = diagnostics();
  const games = (found.apps ?? []).filter((app): app is App & { uuid: string } => !!app.uuid).map(gameEntry);
  const devices = (found.clients ?? []).map(deviceEntry);

  if (!words.length) {
    const catalog = new Map<string, Entry>(
      [...PAGES, ...actions, ...checks, ...games, ...devices, ...schema.settings.map(settingEntry)].map((entry) => [entry.id, entry]),
    );
    const recent = [...new Set(context.recentIds ?? [])]
      .map((id) => catalog.get(id))
      .filter((entry): entry is Entry => Boolean(entry))
      .slice(0, EMPTY_LIMITS.recent);
    const seen = new Set(recent.map((entry) => entry.id));
    const unique = (list: Entry[]) => list.filter((entry) => !seen.has(entry.id));
    const emptyGroups: Group[] = [];
    const topGames = unique(games).slice(0, EMPTY_LIMITS.games);
    const topDevices = unique(devices).slice(0, EMPTY_LIMITS.devices);
    const topPages = unique(PAGES.filter((entry) => primary.some((item) => entry.id === `page:${item.path}`)));
    const topActions = unique(actions);
    const topChecks = unique(checks);
    if (recent.length) emptyGroups.push({ id: 'recent', title: 'Recent', entries: recent });
    if (topGames.length) emptyGroups.push({ id: 'games', title: 'Games', entries: topGames });
    if (topDevices.length) emptyGroups.push({ id: 'devices', title: 'Devices', entries: topDevices });
    if (topPages.length) emptyGroups.push({ id: 'pages', title: 'Pages', entries: topPages });
    if (topActions.length) emptyGroups.push({ id: 'actions', title: 'Host actions', entries: topActions });
    if (topChecks.length) emptyGroups.push({ id: 'diagnostics', title: 'Diagnostics', entries: topChecks });
    return emptyGroups;
  }

  const pages = PAGES.filter((entry) => !DIAGNOSTIC_PATHS.some((path) => entry.id === `page:${path}`));
  const groups: Group[] = [
    { id: 'pages', title: 'Pages', entries: ranked(pages, words).slice(0, PALETTE_LIMITS.pages) },
    { id: 'actions', title: 'Host actions', entries: ranked(actions, words).slice(0, PALETTE_LIMITS.actions) },
    { id: 'diagnostics', title: 'Diagnostics', entries: ranked(checks, words).slice(0, PALETTE_LIMITS.diagnostics) },
    { id: 'games', title: 'Games', entries: ranked(games, words).slice(0, PALETTE_LIMITS.games) },
    { id: 'devices', title: 'Devices', entries: ranked(devices, words).slice(0, PALETTE_LIMITS.devices) },
  ];
  const closest = groups
    .flatMap((group, at) => (group.entries[0] ? [{ group, at, rank: score(group.entries[0], words) }] : []))
    .sort((a, b) => a.rank - b.rank || a.at - b.at)
    .map(({ group }) => group);
  const settings = rankedSettings(searchSettings(schema.settings, categoriesOf, query).map(settingEntry), words, query);
  return settings.length ? [...closest, { id: 'settings', title: 'Settings', entries: settings }] : closest;
}

/** The word under Enter in the legend: an action runs, everything else opens. */
export function enterVerb(entry: Entry | undefined): 'Run' | 'Open' {
  return entry?.action ? 'Run' : 'Open';
}
