// The quick switcher's results (Ctrl+K, QuickSwitcher.svelte): pages, actions, games, devices and
// settings for one query. Pure, so QuickSwitch.test.mjs checks the ranking without a browser.
import type { IconName } from '../components/Icon.svelte';
import { LAUNCHER_NAMES, launcherOf } from '../components/library/selection.svelte';
import { matchesWords, searchSettings, searchWords } from '../components/settings/search';
import type { App, Client } from './api';
import { advanced, primary, type NavItem } from './navigation';
import { categoriesOf, schema, settingGroups, settingsGroup } from './settings-schema';
import type { Setting } from './settings-types';

/** What an action does when chosen; QuickSwitcher.svelte runs it. */
export type ActionId = 'sync' | 'restart' | 'bundle';

export interface Entry {
  /** Unique among all results. */
  id: string;
  label: string;
  /** A second, quieter line: where a setting is, a game's launcher, a device's state. */
  detail?: string;
  /** Words it is found by that it doesn't show. */
  keywords?: string;
  icon: IconName;
  /** Where choosing it goes; otherwise it runs `action`. */
  href?: string;
  action?: ActionId;
  /** A game's cover, drawn in place of the icon. */
  cover?: { uuid: string; path: string };
  /** A device: its row on the Devices page is brought into view. */
  reveal?: string;
}

export interface Group {
  id: 'pages' | 'actions' | 'games' | 'devices' | 'settings';
  title: string;
  entries: Entry[];
}

/** How many of each the list shows, so one long group never hides the others. */
export const LIMITS = { pages: 6, actions: 4, games: 6, devices: 4, settings: 6 } as const;

export const ACTIONS: readonly Entry[] = [
  { id: 'action:pair', label: 'Pair a device', icon: 'link', href: '/devices?pair=qr', keywords: 'add phone tablet tv qr code pin new' },
  { id: 'action:sync', label: 'Sync games now', icon: 'refresh', action: 'sync', keywords: 'launchers steam epic gog playnite library refresh' },
  { id: 'action:restart', label: 'Restart Rubylight', icon: 'power', action: 'restart', keywords: 'reboot reload' },
  { id: 'action:bundle', label: 'Download support bundle', icon: 'download', action: 'bundle', keywords: 'logs crash report bug' },
];

const page = (item: NavItem, detail?: string): Entry => ({
  id: `page:${item.path}`,
  label: item.label,
  detail,
  keywords: item.keywords,
  icon: item.icon,
  href: item.path,
});

/** Every page: the sidebar's, then the two ways to add a game. */
export const PAGES: readonly Entry[] = [
  ...primary.map((item) => page(item)),
  ...advanced.map((item) => page(item, 'Advanced')),
  { id: 'page:/library/import', label: 'Add games', detail: 'Games', icon: 'plus', href: '/library/import', keywords: 'import launcher library' },
  { id: 'page:/library/new', label: 'Add a program', detail: 'Games', icon: 'plus', href: '/library/new', keywords: 'manually app command exe' },
];

/** The link that opens a setting's row in All settings, on whichever tab holds it. */
export function settingHref(setting: Pick<Setting, 'key' | 'category'>): string {
  return `/settings/${settingsGroup(setting.category)}?setting=${encodeURIComponent(setting.key)}`;
}

/** The link to a setting by its key, on whichever tab holds it now. */
export function settingLink(key: string): string {
  const setting = schema.settings.find((candidate) => candidate.key === key);
  return setting ? settingHref(setting) : settingsSearchHref(key);
}

/** All settings' search for a query, as the Settings home links it. */
export function settingsSearchHref(query: string): string {
  return `/settings/${settingGroups[0]?.id ?? 'general'}?q=${encodeURIComponent(query.trim())}`;
}

export function settingEntry(setting: Setting): Entry {
  const tab = categoriesOf(setting)[0]?.label;
  return {
    id: `setting:${setting.key}`,
    label: setting.label,
    detail: [tab, setting.group].filter(Boolean).join(' › '),
    icon: 'settings',
    href: settingHref(setting),
  };
}

export function gameEntry(app: App & { uuid: string }): Entry {
  const launcher = launcherOf(app);
  const notInstalled = app['not-installed'] === true;
  return {
    id: `game:${app.uuid}`,
    label: app.name || 'Unnamed game',
    detail: notInstalled ? 'Not installed' : launcher ? LAUNCHER_NAMES[launcher] : undefined,
    keywords: launcher ? LAUNCHER_NAMES[launcher] : undefined,
    icon: 'library',
    href: `/library/${encodeURIComponent(app.uuid)}`,
    cover: { uuid: app.uuid, path: typeof app['image-path'] === 'string' ? app['image-path'] : '' },
  };
}

export function deviceEntry(client: Client): Entry {
  return {
    id: `device:${client.uuid}`,
    label: client.name,
    detail: !client.enabled ? 'Disabled' : client.connected ? 'Connected' : 'Not connected',
    icon: 'devices',
    href: '/devices',
    reveal: client.name,
  };
}

/**
 * How well an entry matches, lower first: the whole name, its start, the start of a word in it,
 * anywhere in it, then only its detail or keywords.
 */
export function score(entry: Entry, words: readonly string[]): number {
  const label = entry.label.toLowerCase();
  const whole = words.join(' ');
  if (label === whole) return 0;
  if (label.startsWith(whole)) return 1;
  if (startsWord(label, words[0] ?? '') && matchesWords(label, words)) return 2;
  if (matchesWords(label, words)) return 3;
  return 4;
}

const startsWord = (text: string, word: string) => text.split(/[\s\-:’'(),.›]+/).some((part) => part.startsWith(word));

/**
 * Every word is in the name, or starts a word of the detail or keywords: "po" finds Portal 2 and
 * Base port, but not Help for the "po" inside its keyword "support".
 */
export function matches(entry: Entry, words: readonly string[]): boolean {
  const label = entry.label.toLowerCase();
  const rest = [entry.detail, entry.keywords].filter(Boolean).join(' ').toLowerCase();
  return words.every((word) => label.includes(word) || startsWord(rest, word));
}

/** The entries the words find, best first; ties keep their order. */
export function ranked(entries: readonly Entry[], words: readonly string[]): Entry[] {
  return entries
    .filter((entry) => matches(entry, words))
    .map((entry, at) => ({ entry, at, rank: score(entry, words) }))
    .sort((a, b) => a.rank - b.rank || a.at - b.at)
    .map(({ entry }) => entry);
}

/**
 * The results for a query, in groups: the group with the closest match first (on a tie pages,
 * actions, games, then devices) and settings last, so Enter opens the closest match. With no
 * query, the everyday pages and the actions. Games and devices are null until they load.
 */
export function results(query: string, found: { apps: App[] | null; clients: Client[] | null }): Group[] {
  const words = searchWords(query);
  if (!words.length) {
    return [
      { id: 'pages', title: 'Pages', entries: primary.map((item) => page(item)) },
      { id: 'actions', title: 'Actions', entries: [...ACTIONS] },
    ];
  }
  const games = (found.apps ?? []).filter((app): app is App & { uuid: string } => !!app.uuid).map(gameEntry);
  const devices = (found.clients ?? []).map(deviceEntry);
  // Settings are found as All settings finds them, then ranked by their names.
  const settings = searchSettings(schema.settings, categoriesOf, query).map(settingEntry);
  const groups: Group[] = [
    { id: 'pages', title: 'Pages', entries: ranked(PAGES, words).slice(0, LIMITS.pages) },
    { id: 'actions', title: 'Actions', entries: ranked(ACTIONS, words).slice(0, LIMITS.actions) },
    { id: 'games', title: 'Games', entries: ranked(games, words).slice(0, LIMITS.games) },
    { id: 'devices', title: 'Devices', entries: ranked(devices, words).slice(0, LIMITS.devices) },
  ];
  // Each group's entries are ranked already: its first is its closest.
  const closest = groups
    .flatMap((group, at) => (group.entries[0] ? [{ group, at, rank: score(group.entries[0], words) }] : []))
    .sort((a, b) => a.rank - b.rank || a.at - b.at)
    .map(({ group }) => group);
  const listed = rankedSettings(settings, words, query);
  return listed.length ? [...closest, { id: 'settings', title: 'Settings', entries: listed }] : closest;
}

/** The best settings, and when there are more, one entry that opens All settings' search for all of them. */
function rankedSettings(settings: Entry[], words: readonly string[], query: string): Entry[] {
  const sorted = settings
    .map((entry, at) => ({ entry, at, rank: score(entry, words) }))
    .sort((a, b) => a.rank - b.rank || a.at - b.at)
    .map(({ entry }) => entry);
  if (sorted.length <= LIMITS.settings) return sorted;
  return [
    ...sorted.slice(0, LIMITS.settings - 1),
    {
      id: 'settings:all',
      label: `All ${sorted.length} settings for “${query.trim()}”`,
      detail: 'All settings',
      icon: 'search',
      href: settingsSearchHref(query),
    },
  ];
}
