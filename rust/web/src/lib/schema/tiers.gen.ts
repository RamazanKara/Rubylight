// Generated from rust/core/config/settings.toml by tools/generated_docs.py; do not edit by hand.
import type { Setting } from '../settings-types';

export interface MainSection {
  id: string;
  title: string;
  description: string;
  keys: readonly string[];
}

export const MAIN_SECTIONS: readonly MainSection[] = [
  {
    id: 'pc',
    title: 'This PC',
    description: 'Its name, waking it from a device, and updates.',
    keys: ['sunshine_name', 'updates'],
  },
  {
    id: 'picture',
    title: 'Picture',
    description: 'Stream quality, resolution and what happens to your screens while you play.',
    keys: ['stream_preset', 'virtual_display_mode', 'dd_resolution_option', 'virtual_display_layout'],
  },
  {
    id: 'sound',
    title: 'Sound & controls',
    description: 'What your devices can hear and use on this PC.',
    keys: ['stream_audio', 'stream_mic', 'controller'],
  },
  {
    id: 'games',
    title: 'Games',
    description: 'What happens to a game when you stop playing.',
    keys: ['terminate_on_pause'],
  },
];

export const UPDATE_KEYS: readonly string[] = ['updates'];
export const MAIN_KEYS: readonly string[] = MAIN_SECTIONS.flatMap((section) => section.keys);
export const MAIN_BUDGET = 12;

export const ALWAYS_ON: readonly string[] = ['qr_pairing', 'client_roaming', 'bandwidth_probe', 'audio_passthrough', 'audio_surround_714', 'idd_push_fence_ring', 'wgc_direct_encoder_input', 'gpu_compute_conversion', 'packed_444', 'pyrowave', 'hdr_dolby_vision', 'hdr_hlg', 'wgc_pacing_smoothing', 'stream_reconfigure', 'frame_warp', 'keyframe_loss_pacing', 'nvenc_frame_budget_guard', 'nvenc_realtime_hags'];
export const DIAGNOSTICS: readonly string[] = ['capture_poll_interval_us', 'capture_predictive_poll', 'wgc_slot_aligned_publish', 'video_max_batch_size_kb', 'pyrowave_encode_overlap'];
export const DEBUG_GROUP = 'Debug switches';
export const ORIGIN_CATEGORY: Readonly<Record<string, Setting['category']>> = { qr_pairing: 'network' };

export const TRAILING_GROUPS: ReadonlySet<string> = new Set(['Mode substitutions', 'Key remapping', 'Capture troubleshooting', 'AMD AMF', 'NVIDIA NVENC', 'RTSS', 'Ports and addresses', 'Console', 'Debug switches']);
export const FOLDED_GROUPS: ReadonlySet<string> = new Set(['HDR', 'Frame pacing', 'Frame limiter', 'After a stream', 'Capture', 'Remote monitors', 'Steam options', 'Epic Games options', 'Battle.net options', 'Microsoft Store and Xbox options', 'EA app options', 'Ubisoft Connect options', 'GOG options', 'Playnite', 'Playnite launch focus', 'Game folders', 'Lossless Scaling', 'Steam Deck and back grips', 'Bitrate and network', 'Keyboard', 'Mouse, pen and touch', 'Hardware metrics', 'Stream DVR and clips']);
export const GAMES_PAGE_KEYS: readonly string[] = ['steam_enabled', 'steam_include_tools', 'steam_exclude_games', 'library_epic_enabled', 'epic_exclude_games', 'library_battlenet_enabled', 'battlenet_exclude_games', 'library_microsoft_enabled', 'microsoft_exclude_games', 'library_ea_enabled', 'ea_exclude_games', 'library_ubisoft_enabled', 'ubisoft_exclude_games', 'library_gog_enabled', 'gog_exclude_games'];
export const GAMES_PAGE_KEY_SET: ReadonlySet<string> = new Set(GAMES_PAGE_KEYS);
