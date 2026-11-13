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
      'Adds your Playnite games, starts them through Playnite and ends the stream when the game closes. Needs the Rubylight plugin in Playnite; off, the games it added no longer start.',
    details:
      'Install the plugin from Games › Install plugin. Off, an app linked to a Playnite game starts nothing, because Playnite starts it and it has no command of its own; the Playnite sync stops and the Playnite fullscreen app is removed. Games start with the stream’s settings, and the stream ends when Playnite reports the game closed.',
    category: 'library',
    group: 'Playnite',
    control: { kind: 'toggle' },
    default: true,
  },
  {
    key: 'playnite_auto_sync',
    label: 'Add games automatically',
    description:
      'While Playnite runs, adds the games chosen below and removes them when they no longer qualify. Off, no game is added or removed; Sync now only updates those already there.',
    details:
      'While this is on and the Rubylight plugin is installed, the games of every launcher come through Playnite, and Steam’s and the other launchers’ own syncs stand aside so no game is added twice. It syncs when Playnite starts, when these settings change and every ten minutes. Off, Sync now on the Games page still brings the name and cover of the games already added up to date.',
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
    description:
      'Adds every installed game in Playnite, as every launcher does. Turn it off for a shorter list: only recently played games and the categories and libraries below are added.',
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
    description:
      'Also adds the installed games in these Playnite categories, as a JSON list of names: ["Couch", "Racing"]. Has an effect only while Every installed game is off.',
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
    description:
      'Also adds the installed games that come from these Playnite library plugins, as a JSON list of plugin IDs. Has an effect only while Every installed game is off.',
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
    description:
      'A JSON list of Playnite game IDs the sync never adds. Deleting a game on the Games page keeps it out too, with no list to write.',
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
    description:
      'When newer recent games are added, removes as many older added games, so the list keeps its size. Off, the older ones stay and the list grows.',
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
      'After a game closes, the stream shows the desktop this long, then ends. Lower it to leave sooner; too low can end it while a game restarts itself or starts through a launcher.',
    details:
      'A game that restarts itself, or starts through a launcher, is briefly reported closed, and a shorter wait can end its stream then; raise it if a stream ends while a game is still starting. 0 ends the stream at once. Counts from when Playnite reports the game closed and none of its programs is running. This is not the exit timeout, which is how long a game gets to close when Rubylight ends it. 0 to 300 seconds. Applies to the next game started.',
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
      'Puts a launched game or Playnite menu in front, so input reaches it, until confirmed there this many times. Lower it if that interrupts you; a launcher may then take focus back.',
    details:
      '0 leaves focus alone. After launch, Rubylight checks once a second and brings the game or Playnite fullscreen menu to the front until it has been confirmed in front this many times. Launchers and splash screens take the front back while a game starts, so one confirmation is not always enough.',
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
      'The longest Rubylight keeps bringing a launched game or Playnite menu to the front. Raise it if a slow launch leaves the game behind another window; 0 leaves focus alone.',
    details:
      '0 leaves focus alone, even when Confirm game focus is above 0. Applies to a launched game and to the Playnite fullscreen menu; Rubylight checks once a second until this time is over.',
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
      'Stops focusing the game or Playnite menu once it was confirmed in front. Turn it on if launches keep interrupting your window; a launcher or splash screen may then take focus back.',
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
      description: `Adds the games installed with ${launcher.installer} to the library${launcher.art}, and ends a stream when the game exits. Off adds no more games; added ones stay.`,
      category: 'library',
      group,
      control: { kind: 'toggle' },
      default: true,
    },
    {
      key: `${id}_auto_sync`,
      label: 'Sync automatically',
      description: `Checks every five minutes for games installed${launcher.history ? ', removed or played' : ' or removed'}, and updates the library. Off, the list changes only when you press Sync now on the Games page.`,
      details:
        'While Playnite is on with Add games automatically and its Rubylight plugin installed, this launcher’s games come through Playnite and this sync stands aside, so no game is added twice.',
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
        description:
          'Adds every installed game, as every launcher does. Turn it off for a shorter list: only the most recently played games are added, and others the sync added are removed.',
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
      description: `Removes a game’s app when the game is uninstalled; off, it stays in the list.${launcher.history ? ' With recent games only, apps that fall out of the list are always removed.' : ' Games you added by hand always stay.'}`,
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
      details: 'Deleting a game on the Games page keeps it out as well, with no list to write, until you restore it there.',
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
    description:
      'Closes the running app when its last streaming device disconnects; unsaved progress may be lost. Leave it off to keep the game running so you can reconnect and carry on.',
    details: 'Applies when the app uses the global default; each app can override this under Behavior. Takes effect the next time an app starts.',
    category: 'library',
    group: 'App behavior',
    control: { kind: 'toggle' },
    default: false,
    cautionWhen: (values) =>
      on(values, 'terminate_on_pause', false)
        ? 'Unsaved progress may be lost when the last device disconnects.'
        : undefined,
  },
  {
    key: 'exit_timeout',
    label: 'Exit timeout',
    description:
      'How long an app gets to close by itself when Rubylight ends it, before it is ended by force; 0 ends it at once. Raise it if a game needs longer to shut down cleanly.',
    details: 'Applies to every app without its own exit timeout under Behavior, including games added by the Steam and Playnite syncs. 0 to 300 seconds. It is not how soon a stream ends after you quit a game yourself; for Playnite games that is Wait after a game closes.',
    category: 'library',
    group: 'App behavior',
    control: { kind: 'number', min: 0, max: 300, step: 1, unit: 's' },
    default: 10,
    cautionWhen: (values) =>
      String(values.exit_timeout ?? '').trim() === '0'
        ? 'Apps are ended by force at once, with no time to close by themselves.'
        : undefined,
    advanced: true,
  },
  {
    key: 'steam_enabled',
    label: 'Steam library',
    description:
      'Adds your Steam games to the library, with their covers, and ends a stream when the game it started exits. Off adds no more games; added ones stay.',
    category: 'library',
    group: 'Steam options',
    control: { kind: 'toggle' },
    default: true,
  },
  {
    key: 'steam_auto_sync',
    label: 'Sync automatically',
    description:
      'Checks Steam every 30 seconds and updates the Steam apps when games are installed, removed or played. Off, the list changes only when you press Sync now on the Games page.',
    details:
      'While Playnite is on with Add games automatically and its Rubylight plugin installed, the Steam games come through Playnite and this sync stands aside, so no game is added twice.',
    category: 'library',
    group: 'Steam options',
    control: { kind: 'toggle' },
    default: true,
    inactiveHint: "Turn on Steam library to use this setting. Sync now on the Games page applies it without automatic syncing.",
    visibleWhen: steam,
  },
  {
    key: 'steam_sync_all_installed',
    label: 'Every installed game',
    description:
      'Adds every installed game, as every launcher does. Turn it off for a shorter list: only the most recently played games are added, and others the sync added are removed.',
    category: 'library',
    group: 'Steam options',
    control: { kind: 'toggle' },
    default: true,
    inactiveHint: "Turn on Steam library to use this setting. Sync now on the Games page applies it without automatic syncing.",
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
      'Removes a Steam app when its game is uninstalled; off, it stays in the list. With recent games only, apps that fall out of the list are always removed.',
    category: 'library',
    group: 'Steam options',
    control: { kind: 'toggle' },
    default: true,
    inactiveHint: "Turn on Steam library to use this setting. Sync now on the Games page applies it without automatic syncing.",
    visibleWhen: steam,
  },
  {
    key: 'steam_include_tools',
    label: 'Include tools',
    description:
      'Also adds what Steam lists as installed besides games: tools, runtimes, redistributables and DLC. Leave it off unless you want to start one of them from a device.',
    category: 'library',
    group: 'Steam options',
    control: { kind: 'toggle' },
    default: false,
    inactiveHint: "Turn on Steam library to use this setting. Sync now on the Games page applies it without automatic syncing.",
    visibleWhen: steam,
    advanced: true,
  },
  {
    key: 'steam_exclude_games',
    label: 'Games to leave out',
    description:
      'A JSON list of games the sync never adds, by Steam app ID or name: ["570", {"name": "Dota 2"}]. Their Steam apps are removed.',
    details: 'Deleting a game on the Games page keeps it out as well, with no list to write, until you restore it there.',
    category: 'library',
    group: 'Steam options',
    control: { kind: 'json' },
    default: [],
    inactiveHint: "Turn on Steam library to use this setting. Sync now on the Games page applies it without automatic syncing.",
    visibleWhen: steam,
  },
  // Every other launcher has Steam's options, in its own group beside Steam's.
  ...LAUNCHERS.flatMap(launcherSettings),
  ...playniteSettings,
  {
    key: 'library_local_folders',
    label: 'Game folders',
    description:
      'Folders that hold one game per subfolder, such as D:\\Games, for games no launcher lists. Each subfolder joins the library with the program that starts it.',
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
    description:
      'Checks the game folders every five minutes and updates their games when they are added or removed. Off, the list changes only when you press Sync now on the Games page.',
    details:
      'While Playnite is on with Add games automatically and its Rubylight plugin installed, this sync stands aside, as every launcher’s does.',
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
    description: 'Removes a game folder’s app when its folder is deleted; off, it stays in the list. Games you added by hand always stay.',
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
    description:
      'Subfolders to skip in your game folders, by name (Tools) or full path (D:\\Games\\Tools). They are never added, and Add games never lists them.',
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
      'LosslessScaling.exe or its folder, for apps that use Lossless Scaling. Leave it empty unless it is not found: empty finds it in Steam libraries and the usual folders.',
    category: 'library',
    group: 'Lossless Scaling',
    control: { kind: 'text', placeholder: 'Found automatically', mono: true, browse: 'executable' },
    default: '',
  },
  {
    key: 'lossless_scaling_legacy_auto_detect',
    label: 'Let Lossless Scaling start by itself',
    description:
      'Lossless Scaling starts scaling by itself (auto scale) and keeps its window open, instead of Rubylight pressing its hotkey. Try it if scaling does not start; apps can override it.',
    details:
      'Off, Rubylight minimizes the Lossless Scaling window, brings the game to the front and presses the hotkey set in Lossless Scaling, up to three times. Each app can choose its own under Lossless Scaling.',
    category: 'library',
    group: 'Lossless Scaling',
    control: { kind: 'toggle' },
    default: false,
  },
  // Guests from paired devices: another device joins the game a device plays (docs/multiclient.md).
  {
    key: 'guest_join',
    label: 'When another device joins the running game',
    description:
      'Whether a paired device that joins your running game becomes a guest with limited control. Off suits one person with several devices; Ask the player lets you approve each guest.',
    details:
      'Off streams it beside the game with the device’s own permissions and asks nobody, as before guests. Ask the player: the device playing, or the console’s stream card, answers within 45 seconds, and no answer refuses the join; a Moonlight client that gives up sooner shows its own timeout. Let in without asking admits it at once. A guest gets the same picture and sound with its own stream and controller, and only what its guest permission allows.',
    category: 'library',
    group: 'Play together',
    control: {
      kind: 'select',
      options: [
        { value: 'off', label: 'Off, stream beside the game' },
        { value: 'ask', label: 'Ask the player' },
        { value: 'allow', label: 'Let in without asking' },
      ],
    },
    default: 'off',
    cautionWhen: (values) =>
      String(values.guest_join ?? '').trim() === 'allow'
        ? 'A paired device that joins becomes a guest at once; nobody is asked.'
        : undefined,
  },
  {
    key: 'guest_permission',
    label: 'What a guest may do',
    description:
      'What a guest may do unless the player chooses otherwise for that guest. Controller suits playing together; Controller, keyboard and mouse also lets a guest operate this PC.',
    details:
      'View only: picture and sound. Controller: a controller of its own, as the next player. Controller, keyboard and mouse: also the keyboard and mouse, one holder at a time, and the clipboard, USB devices and microphone where its device may use them.',
    category: 'library',
    group: 'Play together',
    control: {
      kind: 'select',
      options: [
        { value: 'view', label: 'View only' },
        { value: 'controller', label: 'Controller' },
        { value: 'controller-keyboard-mouse', label: 'Controller, keyboard and mouse' },
      ],
    },
    default: 'controller',
    visibleWhen: (values) => String(values.guest_join ?? 'off').trim() !== 'off',
  },
];
