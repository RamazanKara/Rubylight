// The settings the Rust host reads, with its real defaults. Each part file
// covers some categories.
import type { Category, CategoryId, Schema } from './settings-types';
import { settings as basics } from './schema/basics';
import { settings as video } from './schema/video';
import { settings as display } from './schema/display';
import { settings as library } from './schema/library';
import { settings as extensions } from './schema/extensions';
import { ALWAYS_ON, EXPERT_GROUPS, DEBUG_GROUP, asDebugSwitch } from './schema/main';

// Topics for search: a setting is found by its topic's name too. What people
// see is the settings tabs below, which gather these topics.
const categories: Category[] = [
  { id: 'general', label: 'This PC', description: 'Name, updates, sleep and the tray' },
  { id: 'network', label: 'Network', description: 'Discovery, encryption and sharing' },
  { id: 'video', label: 'Video', description: 'Capture, codecs, bitrate and frame pacing' },
  { id: 'encoders', label: 'Encoders', description: 'Which GPU and encoder compress the video' },
  { id: 'display', label: 'Display', description: 'Which screen streams and virtual displays' },
  { id: 'frame-limiting', label: 'Frame limiting', description: 'Frame rate caps for games' },
  { id: 'hdr', label: 'HDR', description: 'HDR on the streamed display and in the video' },
  { id: 'audio', label: 'Audio', description: 'Sound, speakers and microphone' },
  { id: 'input', label: 'Input', description: 'Keyboard, mouse, controllers and USB' },
  { id: 'commands', label: 'Commands', description: 'Commands run around apps or from a device' },
  { id: 'library', label: 'Games', description: 'Game sources, app behavior and co-op' },
  { id: 'advanced', label: 'Advanced', description: 'Sign-in, logs, compatibility and custom keys' },
];

const basicKeys = new Set([
  'sunshine_name',
  'enable_pairing',
  'system_tray',
  'hide_tray_controls',
  'update_check_interval',
  'update_check_time',
  'auto_update',
  'notify_pre_releases',
  'sleep_after_idle_minutes',
  'stream_preset',
  'max_bitrate',
  'frame_limiter_enable',
  'frame_limiter_fps_limit',
  'output_name',
  'virtual_display_mode',
  'virtual_display_layout',
  'dd_configuration_option',
  'dd_resolution_option',
  'dd_manual_resolution',
  'dd_refresh_rate_option',
  'dd_manual_refresh_rate',
  'dd_hdr_option',
  'stream_audio',
  'audio_capture_mode',
  'audio_sink',
  'stream_mic',
  'keyboard',
  'mouse',
  'controller',
  'gamepad',
  'forward_rumble',
  'terminate_on_pause',
  'enable_discovery',
  'internet_play',
  'webrtc_server',
  'file_transfer',
  'usb_forwarding',
  'origin_web_ui_allowed',
  'playnite_enabled',
  'playnite_auto_sync',
  'playnite_sync_categories',
  'playnite_fullscreen_entry_enabled',
  'steam_enabled',
  'steam_auto_sync',
]);

export const schema: Schema = {
  categories,
  // Always-on features keep their keys, shown only under Debug switches.
  settings: [...basics, ...video, ...display, ...library, ...extensions]
    .filter(setting => !setting.pending || ALWAYS_ON.includes(setting.key))
    .map(setting => (ALWAYS_ON.includes(setting.key) ? asDebugSwitch(setting) : setting))
    .map(setting => ({ ...setting, advanced: !basicKeys.has(setting.key) })),
};

/** The All settings tabs, the same names and order as the sections of the Settings home. */
export type TabId = 'pc' | 'picture' | 'sound' | 'games' | 'network' | 'advanced';

export interface SettingsTab {
  id: TabId;
  label: string;
  description: string;
}

export const settingGroups: SettingsTab[] = [
  { id: 'pc', label: 'This PC', description: 'Name, updates, sleep and wake' },
  { id: 'picture', label: 'Picture', description: 'Quality, screens, HDR and frame rate' },
  { id: 'sound', label: 'Sound & controls', description: 'Sound, microphone, keyboard, mouse and controllers' },
  { id: 'games', label: 'Games', description: 'Game sources, app behavior and commands' },
  { id: 'network', label: 'Network', description: 'Discovery, encryption and sharing with devices' },
  { id: 'advanced', label: 'Advanced', description: 'Sign-in, logs, compatibility and custom keys' },
];

const TAB_IDS: ReadonlySet<string> = new Set(settingGroups.map((tab) => tab.id));

/**
 * The tab a category's settings show under. Also reads a tab's own id, and
 * the ids older links used (/settings/general, /settings/video, …), so they
 * keep opening the tab that now holds those settings.
 */
export function settingsGroup(id: string): TabId {
  if (TAB_IDS.has(id)) return id as TabId;
  switch (id) {
    case 'general': return 'pc';
    case 'video': case 'encoders': case 'frame-limiting': case 'display': case 'hdr': return 'picture';
    case 'audio': case 'input': return 'sound';
    case 'library': case 'commands': return 'games';
    case 'network': return 'network';
    case 'advanced': return 'advanced';
    default: return 'pc';
  }
}

/** The tab a /settings/:section link opens: its own, or the one an older id now means. */
export function tabOf(section: string): SettingsTab {
  return settingGroups.find((tab) => tab.id === settingsGroup(section)) ?? settingGroups[0]!;
}

/** The categories a setting shows under: its tab, then its own topic. */
export function categoriesOf(setting: { category: CategoryId }): { id: string; label: string; description: string }[] {
  return [
    settingGroups.find((group) => group.id === settingsGroup(setting.category)),
    categories.find((category) => category.id === setting.category),
  ].filter((category): category is SettingsTab | Category => !!category);
}

/**
 * The order of each tab's groups: the ones people look for first. Groups not
 * listed follow in the order the schema first uses them; the expert groups,
 * which start folded, come last, Debug switches at the very end.
 */
const GROUP_ORDER: Record<TabId, readonly string[]> = {
  pc: ['Name and language', 'Updates', 'Sleep and wake', 'Privacy', 'Tray', 'Hardware metrics'],
  picture: [
    'Preset', 'Streamed display', 'Resolution and refresh rate', 'HDR', 'RTX HDR', 'Codecs', 'Bitrate and network',
    'Frame pacing', 'Frame limiter', 'Encoder', 'RTX Video enhancement', 'Capture', 'After a stream',
    'Remote monitors',
  ],
  sound: [
    'Sound', 'Audio devices', 'Virtual speakers', 'Microphone', 'Input devices', 'Keyboard',
    'Mouse, pen and touch', 'Controllers', 'Steam Deck and back grips', 'USB devices',
  ],
  games: [
    'App behavior', 'Steam options', 'Epic Games options', 'Battle.net options', 'Microsoft Store and Xbox options',
    'EA app options', 'Ubisoft Connect options', 'GOG options', 'Playnite', 'Playnite launch focus', 'Game folders',
    'Play together', 'Lossless Scaling', 'Commands around every app', 'Commands from devices',
  ],
  network: ['Pairing', 'Discovery and internet', 'Streams', 'WebRTC', 'File transfer', 'Shared folders', 'Clipboard'],
  advanced: ['Logging', 'Compatibility'],
};

/** Sort key of a group within its tab (see GROUP_ORDER); ties keep schema order. */
export function groupRank(tab: TabId, name: string): number {
  if (name === DEBUG_GROUP) return 3000;
  if (EXPERT_GROUPS.has(name)) return 2000;
  const at = GROUP_ORDER[tab].indexOf(name);
  return at === -1 ? 1000 : at;
}

/** The id of a group's section on its tab, for links such as /settings/pc#updates. */
export function groupAnchor(name: string): string {
  return (
    name
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, '-')
      .replace(/^-|-$/g, '') || 'settings'
  );
}
