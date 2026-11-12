// Settings for game library integrations.
import type { ConfigValue } from '../api';
import type { Setting } from '../settings-types';
import { toList } from '../../components/settings/values';

type Values = Record<string, ConfigValue>;

/** A switch as the host reads it, including the spellings Vibepollo wrote. */
function on(values: Values, key: string, fallback: boolean): boolean {
  const value = values[key] ?? fallback;
  if (typeof value === 'boolean') return value;
  const word = String(value).trim().replace(/^"(.*)"$/, '$1').toLowerCase();
  if (['true', 'yes', '1', 'enable', 'enabled', 'on'].includes(word)) return true;
  if (['false', 'no', '0', 'disable', 'disabled', 'off'].includes(word)) return false;
  return fallback;
}

const steam = (values: Values) => on(values, 'steam_enabled', true);
const recent = (values: Values) => steam(values) && !on(values, 'steam_sync_all_installed', true);
const folders = (values: Values) => toList(values.library_local_folders).some((folder) => folder.trim() !== '');
const playnite = (values: Values) => on(values, 'playnite_enabled', true);
const playniteSync = (values: Values) => playnite(values) && on(values, 'playnite_auto_sync', true);
const playniteRecent = (values: Values) => playniteSync(values) && !on(values, 'playnite_sync_all_installed', true);

const playniteSettings: Setting[] = [
  {
    key: 'playnite_enabled',
    label: 'Playnite',
    description:
      'Starts apps linked to a Playnite game through Playnite and ends the stream when the game closes. Needs the Rubylight plugin in Playnite.',
    details:
      'Games start with the stream’s settings, and the stream ends when Playnite reports the game closed. Install the plugin from Library › Install plugin.',
    category: 'library',
    group: 'Playnite',
    control: { kind: 'toggle' },
    default: true,
  },
  {
    key: 'playnite_auto_sync',
    label: 'Add games automatically',
    description:
      'While Playnite runs, adds the games chosen below as apps and removes them when they no longer qualify. When off, only apps already linked to a game are updated.',
    category: 'library',
    group: 'Playnite',
    control: { kind: 'toggle' },
    default: true,
    inactiveHint: "Turn on Playnite to use this setting. The Rubylight plugin must be installed in Playnite.",
    visibleWhen: playnite,
  },
  {
    key: 'playnite_sync_all_installed',
    label: 'Every installed game',
    description: 'Adds every installed game, as every launcher does. When off, the recently played games and the categories and plugins below are added.',
    category: 'library',
    group: 'Playnite',
    control: { kind: 'toggle' },
    default: true,
    inactiveHint: "Turn on Playnite and Add games automatically to use this setting.",
    visibleWhen: playniteSync,
  },
  {
    key: 'playnite_recent_games',
    label: 'Recent games',
    description: 'How many of the most recently played games to add. 0 adds none.',
    category: 'library',
    group: 'Playnite',
    control: { kind: 'number', min: 0, max: 200, step: 1 },
    default: 10,
    inactiveHint: "Turn on Playnite and Add games automatically, then turn off Every installed game to configure recent games.",
    visibleWhen: playniteRecent,
  },
  {
    key: 'playnite_recent_max_age_days',
    label: 'Played within',
    description: 'Recent games must have been played within this many days. 0 counts any game, played or not.',
    category: 'library',
    group: 'Playnite',
    control: { kind: 'number', min: 0, max: 3650, step: 1, unit: 'days' },
    default: 30,
    inactiveHint: "Turn on Playnite and Add games automatically, then turn off Every installed game to configure recent games.",
    visibleWhen: playniteRecent,
  },
  {
    key: 'playnite_sync_categories',
    label: 'Categories to add',
    description: 'A JSON list of Playnite category names whose installed games are added: ["Couch", "Racing"].',
    category: 'library',
    group: 'Playnite',
    control: { kind: 'json' },
    default: [],
    inactiveHint: "Turn on Playnite and Add games automatically to use this setting.",
    visibleWhen: playniteSync,
  },
  {
    key: 'playnite_sync_plugins',
    label: 'Libraries to add',
    description: 'A JSON list of Playnite library plugin IDs whose installed games are added.',
    category: 'library',
    group: 'Playnite',
    control: { kind: 'json' },
    default: [],
    inactiveHint: "Turn on Playnite and Add games automatically to use this setting.",
    visibleWhen: playniteSync,
    advanced: true,
  },
  {
    key: 'playnite_exclude_games',
    label: 'Games to leave out',
    description: 'A JSON list of Playnite game IDs the sync never adds.',
    category: 'library',
    group: 'Playnite',
    control: { kind: 'json' },
    default: [],
    inactiveHint: "Turn on Playnite and Add games automatically to use this setting.",
    visibleWhen: playniteSync,
  },
  {
    key: 'playnite_exclude_categories',
    label: 'Categories to leave out',
    description: 'A JSON list of category names whose games the sync never adds.',
    category: 'library',
    group: 'Playnite',
    control: { kind: 'json' },
    default: [],
    inactiveHint: "Turn on Playnite and Add games automatically to use this setting.",
    visibleWhen: playniteSync,
    advanced: true,
  },
  {
    key: 'playnite_exclude_plugins',
    label: 'Libraries to leave out',
    description: 'A JSON list of library plugin IDs whose games the sync never adds.',
    category: 'library',
    group: 'Playnite',
    control: { kind: 'json' },
    default: [],
    inactiveHint: "Turn on Playnite and Add games automatically to use this setting.",
    visibleWhen: playniteSync,
    advanced: true,
  },
  {
    key: 'playnite_autosync_remove_uninstalled',
    label: 'Remove uninstalled games',
    description: 'Removes an added app when its game is uninstalled.',
    category: 'library',
    group: 'Playnite',
    control: { kind: 'toggle' },
    default: true,
    inactiveHint: "Turn on Playnite and Add games automatically to use this setting.",
    visibleWhen: playniteSync,
  },
  {
    key: 'playnite_autosync_delete_after_days',
    label: 'Remove unplayed after',
    description: 'Removes an added app not played within this many days of being added. 0 keeps it.',
    category: 'library',
    group: 'Playnite',
    control: { kind: 'number', min: 0, max: 3650, step: 1, unit: 'days' },
    default: 14,
    inactiveHint: "Turn on Playnite and Add games automatically to use this setting.",
    visibleWhen: playniteSync,
  },
  {
    key: 'playnite_autosync_require_replacement',
    label: 'Replace older recent games',
    description: 'When more recent games are played, removes as many older added games as are newly added, keeping the count.',
    category: 'library',
    group: 'Playnite',
    control: { kind: 'toggle' },
    default: true,
    inactiveHint: "Turn on Playnite and Add games automatically, then turn off Every installed game to configure recent games.",
    visibleWhen: playniteRecent,
    advanced: true,
  },
  {
    key: 'playnite_fullscreen_entry_enabled',
    label: 'Playnite fullscreen app',
    description: 'Adds a “Playnite (Fullscreen)” app that opens Playnite’s fullscreen mode, for choosing a game on the device.',
    category: 'library',
    group: 'Playnite',
    control: { kind: 'toggle' },
    default: false,
    inactiveHint: "Turn on Playnite to use this setting. The Rubylight plugin must be installed in Playnite.",
    visibleWhen: playnite,
  },
  {
    key: 'playnite_exit_wait_secs',
    label: 'Wait after a game closes',
    description:
      'How long a game must stay closed before its stream ends, which shows the desktop meanwhile. 0 ends the stream at once.',
    details:
      'Counts from when Playnite reports the game closed and none of its programs is running. Lower it to leave the desktop sooner after quitting a game. A game that restarts itself, or starts through a launcher, is briefly reported closed, and a shorter wait can end its stream then. This is not the exit timeout, which is how long a game gets to close when Rubylight ends it. 0 to 300 seconds. Applies to the next game started.',
    category: 'library',
    group: 'Playnite',
    control: { kind: 'number', min: 0, max: 300, step: 1, unit: 's' },
    default: 15,
    inactiveHint: "Turn on Playnite to use this setting. The Rubylight plugin must be installed in Playnite.",
    visibleWhen: playnite,
    advanced: true,
  },
  {
    key: 'playnite_focus_attempts',
    label: 'Confirm game focus',
    description:
      'How many times the game or Playnite fullscreen menu must be confirmed in front after launch. Lower it if Rubylight keeps interrupting another window.',
    details:
      'After launch, Rubylight checks once a second and brings the game or menu to the front until it has been confirmed this many times. 0 leaves focus alone.',
    category: 'library',
    group: 'Playnite launch focus',
    control: { kind: 'number', min: 0, step: 1 },
    default: 3,
    inactiveHint: 'Turn on Playnite to use its launch focus settings.',
    visibleWhen: playnite,
    advanced: true,
  },
  {
    key: 'playnite_focus_timeout_secs',
    label: 'Stop focusing after',
    description:
      'Maximum time spent bringing a launched game or Playnite fullscreen menu to the front. 0 leaves focus alone, even when Confirm game focus is above 0.',
    category: 'library',
    group: 'Playnite launch focus',
    control: { kind: 'number', min: 0, step: 1, unit: 's' },
    default: 15,
    inactiveHint: 'Turn on Playnite to use its launch focus settings.',
    visibleWhen: playnite,
    advanced: true,
  },
  {
    key: 'playnite_focus_exit_on_first',
    label: 'Stop after the first focus confirmation',
    description:
      'Stop as soon as the game or Playnite menu is confirmed in front. Prevents later focus changes, but a launcher or splash screen may then take focus back.',
    details: 'Has no effect when either focus limit is 0.',
    category: 'library',
    group: 'Playnite launch focus',
    control: { kind: 'toggle' },
    default: false,
    inactiveHint: 'Turn on Playnite to use its launch focus settings.',
    visibleWhen: playnite,
    advanced: true,
  },
];

/** A launcher the host syncs as it does Steam, with Steam's options under its own keys. */
interface Launcher {
  /** The host's source key: `library_<id>_enabled`, `<id>_auto_sync`, … */
  id: 'epic' | 'battlenet' | 'microsoft' | 'ea' | 'ubisoft' | 'gog';
  name: string;
  /** "the games installed with …" */
  installer: string;
  /** What art the host takes from the launcher, if any. */
  art: string;
  /** A game of the launcher, for the leave-out example. */
  example: string;
  /** The launcher records when each game was last played (core/src/library.rs keeps_play_history). */
  history?: boolean;
}

/** In the order of the Library's launcher list. */
const LAUNCHERS: Launcher[] = [
  { id: 'epic', name: 'Epic Games', installer: 'the Epic Games Launcher', art: ', with their covers', example: 'Fortnite' },
  { id: 'battlenet', name: 'Battle.net', installer: 'Battle.net', art: '', example: 'Overwatch 2', history: true },
  { id: 'microsoft', name: 'Microsoft Store and Xbox', installer: 'the Xbox app or the Microsoft Store', art: ', with their icons', example: 'Halo Infinite' },
  { id: 'ea', name: 'EA app', installer: 'the EA app', art: '', example: 'Apex Legends' },
  { id: 'ubisoft', name: 'Ubisoft Connect', installer: 'Ubisoft Connect', art: '', example: "Assassin's Creed Mirage" },
  { id: 'gog', name: 'GOG', installer: 'GOG Galaxy or the GOG installer', art: ', with their icons', example: 'Cyberpunk 2077' },
];

/** Each source's settings group, where the Library's options links land. */
export const LAUNCHER_GROUPS: Record<string, string> = {
  steam: 'Steam options',
  ...Object.fromEntries(LAUNCHERS.map((launcher) => [launcher.id, `${launcher.name} options`])),
  local: 'Game folders',
  playnite: 'Playnite',
};

/** Steam's options for one launcher: its switch, automatic sync, removal, recent games where it keeps a play history, and games to leave out. */
function launcherSettings(launcher: Launcher): Setting[] {
  const { id, name } = launcher;
  const group = LAUNCHER_GROUPS[id]!;
  const enabled = (values: Values) => on(values, `library_${id}_enabled`, true);
  const recentOnly = (values: Values) => enabled(values) && !on(values, `${id}_sync_all_installed`, true);
  const inactiveHint = `Turn on ${name} library to use this setting.`;
  const recentHint = `Turn on ${name} library and turn off Every installed game to configure the recent-games list.`;
  const rows: Setting[] = [
    {
      key: `library_${id}_enabled`,
      label: `${name} library`,
      description: `Adds the games installed with ${launcher.installer} to the library${launcher.art}, and ends a stream when the game exits.`,
      category: 'library',
      group,
      control: { kind: 'toggle' },
      default: true,
    },
    {
      key: `${id}_auto_sync`,
      label: 'Sync automatically',
      description: `Checks every five minutes for games installed${launcher.history ? ', removed or played' : ' or removed'}, and updates the library.`,
      category: 'library',
      group,
      control: { kind: 'toggle' },
      default: true,
      inactiveHint,
      visibleWhen: enabled,
    },
  ];
  if (launcher.history) {
    rows.push(
      {
        key: `${id}_sync_all_installed`,
        label: 'Every installed game',
        description: 'Adds every installed game, as every launcher does. When off, only the games played most recently are added.',
        category: 'library',
        group,
        control: { kind: 'toggle' },
        default: true,
        inactiveHint,
        visibleWhen: enabled,
      },
      {
        key: `${id}_recent_games`,
        label: 'Recent games',
        description: 'How many of the most recently played games to add.',
        category: 'library',
        group,
        control: { kind: 'number', min: 0, max: 100, step: 1 },
        default: 10,
        inactiveHint: recentHint,
        visibleWhen: recentOnly,
      },
      {
        key: `${id}_recent_max_age_days`,
        label: 'Played within',
        description: 'Leaves out games not played for this many days. 0 adds them however long ago they were played.',
        category: 'library',
        group,
        control: { kind: 'number', min: 0, max: 3650, step: 1, unit: 'days' },
        default: 30,
        inactiveHint: recentHint,
        visibleWhen: recentOnly,
      },
    );
  }
  rows.push(
    {
      key: `${id}_autosync_remove_uninstalled`,
      label: 'Remove uninstalled games',
      description: `Removes a game’s app when the game is uninstalled.${launcher.history ? ' With recent games only, apps that fall out of the list are always removed.' : ' Games you added by hand stay.'}`,
      category: 'library',
      group,
      control: { kind: 'toggle' },
      default: true,
      inactiveHint,
      visibleWhen: enabled,
    },
    {
      key: `${id}_exclude_games`,
      label: 'Games to leave out',
      description: `A JSON list of games the sync never adds, by name or ${name} ID: [{"name": "${launcher.example}"}]. Apps already added for them are removed.`,
      category: 'library',
      group,
      control: { kind: 'json' },
      default: [],
      inactiveHint,
      visibleWhen: enabled,
    },
  );
  return rows;
}

export const settings: Setting[] = [
  {
    key: 'terminate_on_pause',
    label: 'Close the app when the last device disconnects',
    description: 'Closes the running app when its last streaming device disconnects. Off keeps it running for reconnection.',
    details: 'Applies when the app uses the global default. Each app can override this under Behavior. Takes effect the next time an app starts; unsaved progress may be lost when it closes.',
    category: 'library',
    group: 'App behavior',
    control: { kind: 'toggle' },
    default: false,
  },
  {
    key: 'exit_timeout',
    label: 'Exit timeout',
    description: 'How long an app gets to close when Rubylight ends it, before it is ended by force. 0 ends it at once.',
    details: 'Applies to every app without its own exit timeout under Behavior, including games added by the Steam and Playnite syncs. 0 to 300 seconds. It is not how soon a stream ends after you quit a game yourself; for Playnite games that is Wait after a game closes.',
    category: 'library',
    group: 'App behavior',
    control: { kind: 'number', min: 0, max: 300, step: 1, unit: 's' },
    default: 10,
    advanced: true,
  },
  {
    key: 'steam_enabled',
    label: 'Steam library',
    description:
      'Adds your Steam games to the library, with their covers, and ends a stream when the game it started exits.',
    category: 'library',
    group: 'Steam options',
    control: { kind: 'toggle' },
    default: true,
  },
  {
    key: 'steam_auto_sync',
    label: 'Sync automatically',
    description: 'Checks Steam every 30 seconds and updates the Steam apps when games are installed, removed or played.',
    category: 'library',
    group: 'Steam options',
    control: { kind: 'toggle' },
    default: true,
    inactiveHint: "Turn on Steam library to use this setting. Sync now in Library applies it without automatic syncing.",
    visibleWhen: steam,
  },
  {
    key: 'steam_sync_all_installed',
    label: 'Every installed game',
    description: 'Adds every installed game, as every launcher does. When off, only the games played most recently are added.',
    category: 'library',
    group: 'Steam options',
    control: { kind: 'toggle' },
    default: true,
    inactiveHint: "Turn on Steam library to use this setting. Sync now in Library applies it without automatic syncing.",
    visibleWhen: steam,
  },
  {
    key: 'steam_recent_games',
    label: 'Recent games',
    description: 'How many of the most recently played games to add.',
    category: 'library',
    group: 'Steam options',
    control: { kind: 'number', min: 0, max: 100, step: 1 },
    default: 10,
    inactiveHint: "Turn on Steam library and turn off Every installed game to configure the recent-games list.",
    visibleWhen: recent,
  },
  {
    key: 'steam_recent_max_age_days',
    label: 'Played within',
    description: 'Leaves out games not played for this many days. 0 adds them however long ago they were played.',
    category: 'library',
    group: 'Steam options',
    control: { kind: 'number', min: 0, max: 3650, step: 1, unit: 'days' },
    default: 30,
    inactiveHint: "Turn on Steam library and turn off Every installed game to configure the recent-games list.",
    visibleWhen: recent,
  },
  {
    key: 'steam_autosync_remove_uninstalled',
    label: 'Remove uninstalled games',
    description:
      'Removes a Steam app when its game is uninstalled. With recent games only, apps that fall out of the list are always removed.',
    category: 'library',
    group: 'Steam options',
    control: { kind: 'toggle' },
    default: true,
    inactiveHint: "Turn on Steam library to use this setting. Sync now in Library applies it without automatic syncing.",
    visibleWhen: steam,
  },
  {
    key: 'steam_include_tools',
    label: 'Include tools',
    description: 'Also adds tools, runtimes, redistributables and DLC that Steam lists as installed.',
    category: 'library',
    group: 'Steam options',
    control: { kind: 'toggle' },
    default: false,
    inactiveHint: "Turn on Steam library to use this setting. Sync now in Library applies it without automatic syncing.",
    visibleWhen: steam,
    advanced: true,
  },
  {
    key: 'steam_exclude_games',
    label: 'Games to leave out',
    description:
      'A JSON list of games the sync never adds, by Steam app ID or name: ["570", {"name": "Dota 2"}]. Their Steam apps are removed.',
    category: 'library',
    group: 'Steam options',
    control: { kind: 'json' },
    default: [],
    inactiveHint: "Turn on Steam library to use this setting. Sync now in Library applies it without automatic syncing.",
    visibleWhen: steam,
  },
  // Every other launcher has Steam's options, in its own group beside Steam's.
  ...LAUNCHERS.flatMap(launcherSettings),
  ...playniteSettings,
  {
    key: 'library_local_folders',
    label: 'Game folders',
    description: 'Folders that hold one game per subfolder, such as D:\\Games. Each subfolder joins the library with the program that starts it.',
    details:
      'Installers, uninstallers, redistributables, crash reporters and updaters are skipped, and programs more than two folders down are not read. A game a launcher already lists is shown under that launcher.',
    category: 'library',
    group: 'Game folders',
    control: { kind: 'list', placeholder: 'D:\\Games' },
    default: [],
  },
  {
    key: 'library_local_auto_sync',
    label: 'Sync automatically',
    description: 'Checks the game folders every five minutes and updates their games when they are added or removed.',
    category: 'library',
    group: 'Game folders',
    control: { kind: 'toggle' },
    default: true,
    inactiveHint: 'Add a game folder to use this setting.',
    visibleWhen: folders,
  },
  {
    key: 'library_local_autosync_remove_uninstalled',
    label: 'Remove deleted games',
    description: 'Removes a game folder’s app when its folder is deleted. Games you added by hand stay.',
    category: 'library',
    group: 'Game folders',
    control: { kind: 'toggle' },
    default: true,
    inactiveHint: 'Add a game folder to use this setting.',
    visibleWhen: folders,
  },
  {
    key: 'library_local_exclude',
    label: 'Folders to leave out',
    description: 'Game folders Add games never lists, by name (Tools) or full path (D:\\Games\\Tools).',
    category: 'library',
    group: 'Game folders',
    control: { kind: 'list', placeholder: 'Tools' },
    default: [],
    inactiveHint: 'Add a game folder to use this setting.',
    visibleWhen: folders,
  },
  {
    key: 'lossless_scaling_path',
    label: 'Lossless Scaling program',
    description:
      'LosslessScaling.exe or its folder, for apps that use Lossless Scaling. Empty finds it in Steam libraries and the usual folders.',
    category: 'library',
    group: 'Lossless Scaling',
    control: { kind: 'text', placeholder: 'Found automatically', mono: true, browse: 'executable' },
    default: '',
  },
  {
    key: 'lossless_scaling_legacy_auto_detect',
    label: 'Let Lossless Scaling start by itself',
    description:
      'Uses Lossless Scaling’s auto scale for the game instead of pressing its hotkey, and leaves its window open. An app can choose otherwise.',
    category: 'library',
    group: 'Lossless Scaling',
    control: { kind: 'toggle' },
    default: false,
  },
];
