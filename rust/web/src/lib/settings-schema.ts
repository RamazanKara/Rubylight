// The settings the Rust host reads, with its real defaults. Each part file
// covers some categories.
import type { Category, CategoryId, Schema } from './settings-types';
import { settings as basics } from './schema/basics';
import { settings as video } from './schema/video';
import { settings as display } from './schema/display';
import { settings as library } from './schema/library';

const categories: Category[] = [
  { id: 'general', label: 'General', description: 'Host name, pairing, updates and logs' },
  { id: 'network', label: 'Network', description: 'Ports, encryption and console access' },
  { id: 'video', label: 'Video', description: 'Capture, codecs, bitrate and frame pacing' },
  { id: 'encoders', label: 'Encoders', description: 'Which GPU and encoder compress the video' },
  { id: 'display', label: 'Display', description: 'Which screen streams and virtual displays' },
  { id: 'frame-limiting', label: 'Frame limiting', description: 'Frame rate caps for games' },
  { id: 'hdr', label: 'HDR', description: 'HDR on the streamed display' },
  { id: 'audio', label: 'Audio', description: 'Sound, speakers and microphone' },
  { id: 'input', label: 'Input', description: 'Keyboard, mouse and controllers' },
  { id: 'commands', label: 'Commands', description: 'Commands run around apps or from a device' },
  { id: 'library', label: 'Library', description: 'App behavior, libraries and Lossless Scaling' },
  { id: 'advanced', label: 'Advanced', description: 'Compatibility and custom settings' },
];

const basicKeys = new Set([
  'sunshine_name',
  'enable_pairing',
  'system_tray',
  'hide_tray_controls',
  'update_check_interval',
  'auto_update',
  'notify_pre_releases',
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
  'webrtc_server',
  'origin_web_ui_allowed',
  'playnite_enabled',
  'playnite_auto_sync',
  'playnite_sync_all_installed',
  'playnite_recent_games',
  'playnite_sync_categories',
  'playnite_fullscreen_entry_enabled',
  'steam_enabled',
  'steam_auto_sync',
  'steam_sync_all_installed',
  'steam_recent_games',
]);

export const schema: Schema = {
  categories,
  settings: [...basics, ...video, ...display, ...library].map(setting => ({ ...setting, advanced: !basicKeys.has(setting.key) })),
};

export const settingGroups: Category[] = [
  { id: 'general', label: 'General', description: 'This PC, pairing, updates and compatibility' },
  { id: 'video', label: 'Streaming', description: 'Stream quality and frame rate' },
  { id: 'display', label: 'Display & HDR', description: 'Choose the screen, resolution and HDR behavior' },
  { id: 'audio', label: 'Audio & controls', description: 'Sound, microphone and device controls' },
  { id: 'library', label: 'Games & automation', description: 'Library sources, app behavior and commands' },
  { id: 'network', label: 'Network & access', description: 'Discovery, connections and console access' },
];

export function settingsGroup(id: string): CategoryId {
  switch (id) {
    case 'encoders': case 'frame-limiting': case 'video': return 'video';
    case 'hdr': case 'display': return 'display';
    case 'input': case 'audio': return 'audio';
    case 'commands': case 'library': return 'library';
    case 'network': return 'network';
    default: return 'general';
  }
}
