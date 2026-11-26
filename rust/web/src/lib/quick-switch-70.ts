// The Rubylight 7.0 command palette (Ctrl+K, QuickSwitcher.svelte): quick-switch.ts's search,
// grouped as Pages, Host actions, Diagnostics, Games, Devices and Settings. Host actions know the
// host's state: a theme the console doesn't use yet, and Disconnect every stream only while a
// device streams. Pure, so QuickSwitch70.test.mjs checks it without a browser.
import { searchSettings, searchWords } from '../components/settings/search';
import type { App, Client } from './api';
import { advanced, primary } from './navigation';
import {
  ACTIONS, LIMITS, PAGES, deviceEntry, gameEntry, ranked, rankedSettings, score, settingEntry,
  type Entry, type Group,
} from './quick-switch';
import { categoriesOf, schema } from './settings-schema';
import type { Theme } from './theme';

/** What the palette knows about the console and the host when it opens. */
export interface PaletteContext {
  theme: Theme;
  /** Streams to devices right now. */
  streams: number;
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
const DISPLAYS: Entry = {
  id: 'action:displays', label: 'Restore displays', detail: 'Troubleshoot', icon: 'monitor', href: '/troubleshoot#displays',
  keywords: 'monitor layout virtual display screen black fix saved',
};
const BUNDLE = ACTIONS.find((entry) => entry.action === 'bundle')!;

/** The host actions for this moment: what can't be done now is not offered. */
export function hostActions(context: PaletteContext): Entry[] {
  const themes = (['light', 'dark', 'system'] as Theme[]).filter((theme) => theme !== context.theme).map((theme) => THEMES[theme]);
  return [
    ...ACTIONS.filter((entry) => entry !== BUNDLE && entry.action !== 'restart'),
    ...(context.streams > 0 ? [{ ...STOP, detail: context.streams === 1 ? '1 stream' : `${context.streams} streams` }] : []),
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
export const PALETTE_LIMITS = { ...LIMITS, actions: 5 } as const;

/**
 * The palette for a query. With nothing typed: the everyday pages, the host actions and
 * diagnostics. With a query: the group with the closest match first (on a tie pages, host actions,
 * diagnostics, games, then devices) and settings last, so Enter takes the closest match.
 */
export function palette(query: string, found: { apps: App[] | null; clients: Client[] | null }, context: PaletteContext): Group[] {
  const words = searchWords(query);
  const actions = hostActions(context);
  const checks = diagnostics();
  if (!words.length) {
    return [
      { id: 'pages', title: 'Pages', entries: PAGES.filter((entry) => primary.some((item) => entry.id === `page:${item.path}`)) },
      { id: 'actions', title: 'Host actions', entries: actions },
      { id: 'diagnostics', title: 'Diagnostics', entries: checks },
    ];
  }
  const pages = PAGES.filter((entry) => !DIAGNOSTIC_PATHS.some((path) => entry.id === `page:${path}`));
  const games = (found.apps ?? []).filter((app): app is App & { uuid: string } => !!app.uuid).map(gameEntry);
  const devices = (found.clients ?? []).map(deviceEntry);
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
