// Settings for the categories named in settings-schema.ts.
import type { ConfigValue } from '../api';
import type { Setting } from '../settings-types';

type Values = Record<string, ConfigValue>;

/** A text setting as the host reads it: unset or blank means the default. */
function text(values: Values, key: string, fallback: string): string {
  const value = values[key];
  if (value === undefined || value === null) return fallback;
  const trimmed = String(value).trim();
  return trimmed === '' ? fallback : trimmed;
}

/** A switch as the host reads it, including the spellings Vibepollo wrote. */
function on(values: Values, key: string, fallback: boolean): boolean {
  const value = values[key] ?? fallback;
  if (typeof value === 'boolean') return value;
  const word = String(value).trim().replace(/^"(.*)"$/, '$1').toLowerCase();
  if (['true', 'yes', '1', 'enable', 'enabled', 'on'].includes(word)) return true;
  if (['false', 'no', '0', 'disable', 'disabled', 'off'].includes(word)) return false;
  return fallback;
}

/** Whether the limiter may use RTSS. The host reads unknown providers as automatic. */
function rtss(values: Values): boolean {
  const name = text(values, 'frame_limiter_provider', 'auto')
    .toLowerCase()
    .replace(/[^a-z0-9]/g, '');
  return !['none', 'disabled'].includes(name);
}

const SCALES = [100, 125, 150, 175, 200, 225, 250, 300, 350, 400, 450, 500];

export const settings: Setting[] = [
  // Display
  {
    key: 'virtual_display_mode',
    label: 'Virtual display',
    description:
      'Whether each device gets its own virtual display or all devices share one. A device’s or app’s own choice wins.',
    details: 'Unset, it is one for each device.',
    category: 'display',
    group: 'Streamed display',
    control: {
      kind: 'select',
      options: [
        { value: '', label: 'Automatic (one for each device)' },
        { value: 'per_client', label: 'One for each device' },
        { value: 'shared', label: 'One shared by all devices' },
      ],
    },
    default: '',
  },
  {
    key: 'virtual_display_layout',
    label: 'Virtual display layout',
    description:
      'How the virtual display fits with your other monitors while streaming. Exclusive turns the other monitors off.',
    details:
      'Extended keeps your PC display primary. Making the virtual display primary moves the main desktop and taskbar to the stream. Isolated places the virtual display far from the other monitors so the mouse cannot wander onto it. A Remote Monitor follows exclusive and primary; choose Extended to keep its saved placement.',
    category: 'display',
    group: 'Streamed display',
    control: {
      kind: 'select',
      options: [
        { value: 'exclusive', label: 'Exclusive' },
        { value: 'extended', label: 'Extend — keep PC display primary' },
        { value: 'extended_primary', label: 'Extend — make virtual display primary' },
        { value: 'extended_isolated', label: 'Isolate — keep PC display primary' },
        { value: 'extended_primary_isolated', label: 'Isolate — make virtual display primary' },
      ],
    },
    // display_policy.rs Arrangement::parse: any case, - for _, and the display activation names.
    aliases: {
      match: 'dash',
      spellings: { ensure_active: 'extended', ensure_primary: 'extended_primary', ensure_only_display: 'exclusive' },
    },
    default: 'exclusive',
  },
  {
    key: 'dd_virtual_display_scale',
    label: 'Virtual display scale',
    description:
      'The Windows scale set on a virtual display. Recommended picks one from the resolution, such as 125% at 1080p; Keep leaves the scale Windows remembers for that display.',
    category: 'display',
    group: 'Streamed display',
    control: {
      kind: 'select',
      options: [
        { value: '0', label: 'Keep' },
        { value: '-1', label: 'Recommended' },
        ...SCALES.map((scale) => ({ value: String(scale), label: `${scale}%` })),
      ],
    },
    default: 0,
  },
  {
    key: 'dd_resolution_option',
    label: 'Resolution',
    description:
      'The resolution set on the streamed display. A virtual display matches the device unless a fixed resolution is set.',
    category: 'display',
    group: 'Resolution and refresh rate',
    control: {
      kind: 'select',
      options: [
        { value: 'auto', label: 'Match the device' },
        { value: 'disabled', label: 'Keep the current resolution' },
        { value: 'manual', label: 'Use a fixed resolution' },
      ],
    },
    default: 'auto',
  },
  {
    key: 'dd_manual_resolution',
    label: 'Fixed resolution',
    description: 'WIDTHxHEIGHT, for example 2560x1440. The width can be 320 to 7680 and the height 200 to 4320.',
    category: 'display',
    group: 'Resolution and refresh rate',
    control: { kind: 'text', placeholder: '2560x1440', mono: true },
    default: '',
    inactiveHint: "Choose Use a fixed resolution under Resolution to apply this value.",
    visibleWhen: (values) => text(values, 'dd_resolution_option', 'auto') === 'manual',
    requiredWhen: (values) => text(values, 'dd_resolution_option', 'auto') === 'manual',
  },
  {
    key: 'dd_refresh_rate_option',
    label: 'Refresh rate',
    description: 'The refresh rate set on the streamed display.',
    details:
      'Matching follows Virtual display refresh under Frame limiting.',
    category: 'display',
    group: 'Resolution and refresh rate',
    control: {
      kind: 'select',
      options: [
        { value: 'auto', label: 'Match the stream' },
        { value: 'disabled', label: 'Keep the current refresh rate' },
        { value: 'manual', label: 'Use a fixed refresh rate' },
      ],
    },
    default: 'auto',
  },
  {
    key: 'dd_manual_refresh_rate',
    label: 'Fixed refresh rate',
    description:
      'In hertz, with up to three decimals such as 59.94 or 119.88. It replaces the faster automatic refresh.',
    category: 'display',
    group: 'Resolution and refresh rate',
    control: { kind: 'number', min: 1, max: 1000, step: 0.001, unit: 'Hz' },
    default: '',
    inactiveHint: "Choose Use a fixed refresh rate under Refresh rate to apply this value.",
    visibleWhen: (values) => text(values, 'dd_refresh_rate_option', 'auto') === 'manual',
    requiredWhen: (values) => text(values, 'dd_refresh_rate_option', 'auto') === 'manual',
  },
  {
    key: 'dd_mode_remapping',
    label: 'Mode substitutions',
    description:
      'Changes a requested mode before it is applied. Each entry matches requested_resolution and requested_fps and sets final_resolution and final_refresh_rate.',
    details:
      'The first matching entry is used. The mixed list is used when resolution and refresh rate both match the device, resolution_only or refresh_rate_only when only that one does.',
    category: 'display',
    group: 'Resolution and refresh rate',
    control: { kind: 'json' },
    default: { mixed: [], resolution_only: [], refresh_rate_only: [] },
    inactiveHint: "Choose Match the device for Resolution or Match the stream for Refresh rate to apply substitutions.",
    visibleWhen: (values) =>
      text(values, 'dd_resolution_option', 'auto') === 'auto' ||
      text(values, 'dd_refresh_rate_option', 'auto') === 'auto',
    advanced: true,
  },
  {
    key: 'dd_config_revert_on_disconnect',
    label: 'Restore displays on disconnect',
    description:
      'When a device disconnects but its app keeps running, restore the displays after the delay below instead of keeping them for a reconnect.',
    category: 'display',
    group: 'After a stream',
    control: { kind: 'toggle' },
    default: false,
  },
  {
    key: 'dd_config_revert_delay',
    label: 'Restore delay',
    description: 'How long to wait after a device disconnects before restoring the displays.',
    category: 'display',
    group: 'After a stream',
    control: { kind: 'number', min: 0, step: 100, unit: 'ms' },
    default: 3000,
    inactiveHint: "Turn on Restore displays on disconnect to apply this delay.",
    visibleWhen: (values) => on(values, 'dd_config_revert_on_disconnect', false),
  },
  {
    key: 'dd_paused_virtual_display_timeout_secs',
    label: 'Keep a disconnected display for',
    description:
      'How long a disconnected device’s display is kept while its app runs, so the device can resume. 0 keeps it until the app closes.',
    category: 'display',
    group: 'After a stream',
    control: { kind: 'number', min: 0, step: 1, unit: 's' },
    default: 7200,
    inactiveHint: "Turn off Restore displays on disconnect to keep a disconnected display available for reconnection.",
    visibleWhen: (values) => !on(values, 'dd_config_revert_on_disconnect', false),
  },
  {
    key: 'dd_always_restore_from_golden',
    label: 'Restore the saved layout',
    description:
      'After a stream, return the displays to the layout saved under Maintenance instead of the layout from before the stream.',
    details: 'The saved layout is used only while every display it names is connected.',
    category: 'display',
    group: 'After a stream',
    control: { kind: 'toggle' },
    default: false,
  },
  {
    key: 'dd_snapshot_exclude_devices',
    label: 'Displays the saved layout skips',
    description:
      'A list of display device IDs in JSON format (text enclosed in square brackets) that restoring the saved layout leaves as they are, such as a dummy plug.',
    category: 'display',
    group: 'After a stream',
    control: { kind: 'json' },
    default: [],
    advanced: true,
  },
  {
    key: 'dd_snapshot_restore_hotkey',
    label: 'Restore hotkey',
    description:
      'A key on this PC that ends every stream and restores the display layout, for when a stream leaves the screens unusable. F1 to F24, a letter or a digit.',
    details:
      'It also removes the virtual displays. A virtual-key code such as 0x2E works too. Empty, 0, none, off or disabled turns it off.',
    category: 'display',
    group: 'After a stream',
    control: { kind: 'text', placeholder: 'Off', mono: true },
    default: '',
  },
  {
    key: 'dd_snapshot_restore_hotkey_modifiers',
    label: 'Restore hotkey modifiers',
    description: 'Keys held with the restore hotkey: ctrl, alt, shift and win, joined with +.',
    category: 'display',
    group: 'After a stream',
    control: { kind: 'text', placeholder: 'ctrl+alt+shift', mono: true },
    default: 'ctrl+alt+shift',
    inactiveHint: "Set a Restore hotkey to use these modifiers.",
    // hotkey.rs reads these words (0 is Vibepollo's stored default) as no hotkey.
    visibleWhen: (values) =>
      !['', '0', 'none', 'off', 'disabled'].includes(String(values.dd_snapshot_restore_hotkey ?? '').trim().toLowerCase()),
  },
  {
    key: 'remote_monitor_mute_audio',
    label: 'Mute remote monitors',
    description: 'Sends picture and input, but no audio, to a device used as a remote monitor.',
    category: 'display',
    group: 'Remote monitors',
    control: { kind: 'toggle' },
    default: false,
  },
  {
    key: 'remote_monitor_disconnect_on_stream_end',
    label: 'Remove when its stream ends',
    description:
      'Removes a device’s remote monitor when its stream ends. When off, the monitor stays so the device can resume.',
    category: 'display',
    group: 'Remote monitors',
    control: { kind: 'toggle' },
    default: false,
  },
  {
    key: 'remote_monitor_disconnect_on_client_disconnect',
    label: 'Remove when the device disconnects',
    description:
      'Removes the remote monitor as soon as the device’s connection drops, even if it was kept for resuming.',
    category: 'display',
    group: 'Remote monitors',
    control: { kind: 'toggle' },
    default: false,
  },
  {
    key: 'remote_monitor_terminate_on_first_request',
    label: 'Quit the game on the first request',
    description:
      'Lets another device quit the running game with its first request. When off, it must ask again within 60 seconds; the device that started the game is never asked twice.',
    category: 'display',
    group: 'Remote monitors',
    control: { kind: 'toggle' },
    default: false,
  },
  {
    key: 'remote_monitor_confirm_app_replacement',
    label: 'Confirm before replacing a running app',
    description:
      'Makes a device ask twice before replacing a running app with a different one, so Moonlight can warn first.',
    details:
      'The first request to start a different app while one runs is refused; starting it again within 60 seconds replaces the app. Remote monitors and remote input still connect without asking.',
    category: 'display',
    group: 'Remote monitors',
    control: { kind: 'toggle' },
    default: true,
  },

  // Frame limiting
  {
    key: 'frame_limiter_enable',
    label: 'Limit every stream',
    description:
      'Caps the game’s frame rate on every stream. Streams are already limited unless Virtual display refresh is off.',
    category: 'frame-limiting',
    group: 'Frame limiter',
    control: { kind: 'toggle' },
    default: false,
  },
  {
    key: 'frame_limiter_provider',
    label: 'Limiter',
    description:
      'What enforces the limit. Automatic and RTSS use RivaTuner Statistics Server (RTSS), which must be installed; None turns off all limiting.',
    category: 'frame-limiting',
    group: 'Frame limiter',
    control: {
      kind: 'select',
      options: [
        { value: 'auto', label: 'Automatic' },
        { value: 'rtss', label: 'RTSS' },
        { value: 'none', label: 'None' },
      ],
    },
    // framegen.rs Provider::parse reads letters and digits only.
    aliases: {
      match: 'letters',
      spellings: { disabled: 'none' },
    },
    default: 'auto',
  },
  {
    key: 'frame_limiter_fps_limit',
    label: 'Frame limit',
    description:
      'The frame rate games are capped at; 0 uses the stream’s frame rate. RTSS keeps decimals such as 59.94.',
    category: 'frame-limiting',
    group: 'Frame limiter',
    control: { kind: 'number', min: 0, max: 1000, step: 0.001, unit: 'fps' },
    default: 0,
  },
  {
    key: 'frame_limiter_auto_virtual_framegen',
    label: 'Virtual display refresh',
    description:
      'How fast a virtual display refreshes. A faster display captures each game frame sooner after it is drawn.',
    details: 'Every choice but Off also caps games at the frame limit and uses 1000 Hz when the device asks for VRR.',
    category: 'frame-limiting',
    group: 'Frame limiter',
    control: {
      kind: 'select',
      options: [
        { value: 'legacy', label: 'Twice the stream rate' },
        { value: 'enabled', label: 'Four times the stream rate' },
        { value: 'vrr', label: '1000 Hz' },
        { value: 'disabled', label: 'Off' },
      ],
    },
    // framegen.rs virtual_refresh: Vibepollo's spellings and the fallback console's fixed-1000hz.
    aliases: {
      match: 'letters',
      spellings: {
        '2x': 'legacy',
        'fixed-2x': 'legacy',
        enable: 'enabled',
        true: 'enabled',
        yes: 'enabled',
        on: 'enabled',
        '1': 'enabled',
        smooth: 'enabled',
        smoother: 'enabled',
        'fixed-1000hz': 'vrr',
        '1000hz': 'vrr',
        '1000': 'vrr',
        disable: 'disabled',
        false: 'disabled',
        no: 'disabled',
        off: 'disabled',
        '0': 'disabled',
      },
    },
    default: 'legacy',
  },
  {
    key: 'rtss_install_path',
    label: 'RTSS folder',
    description: 'Where RivaTuner Statistics Server is installed. Empty looks for it in Program Files.',
    category: 'frame-limiting',
    group: 'RTSS',
    control: { kind: 'text', placeholder: 'C:\\Program Files (x86)\\RivaTuner Statistics Server', mono: true, browse: 'directory' },
    default: '',
    inactiveHint: "Choose Automatic or RTSS under Limiter to use RTSS settings. RTSS must be installed on the host.",
    visibleWhen: rtss,
  },
  {
    key: 'rtss_frame_limit_type',
    label: 'RTSS limit mode',
    description: 'How RTSS paces frames. Some streams use front edge sync instead.',
    details:
      'Streams use front edge sync instead, unless Virtual display refresh is off.',
    category: 'frame-limiting',
    group: 'RTSS',
    control: {
      kind: 'select',
      options: [
        { value: 'async', label: 'Asynchronous' },
        { value: 'front edge sync', label: 'Front edge sync' },
        { value: 'back edge sync', label: 'Back edge sync' },
      ],
    },
    // framegen.rs reads letters and digits only, so the fallback console's front-edge-sync is front edge sync.
    aliases: { match: 'letters' },
    default: 'async',
    inactiveHint: "Choose Automatic or RTSS under Limiter to use RTSS settings. RTSS must be installed on the host.",
    visibleWhen: rtss,
  },
  {
    key: 'rtss_allow_virtual_display_override',
    label: 'Use this mode on virtual displays',
    description: 'Applies the RTSS limit mode to virtual-display streams instead of front edge sync.',
    details:
      'Apps set to game-provided frame generation keep front edge sync, because other modes can add a lot of latency there.',
    category: 'frame-limiting',
    group: 'RTSS',
    control: { kind: 'toggle' },
    default: false,
    inactiveHint: "Choose Automatic or RTSS under Limiter to use RTSS settings. RTSS must be installed on the host.",
    visibleWhen: rtss,
    advanced: true,
  },

  // HDR
  {
    key: 'dd_hdr_option',
    label: 'Display HDR',
    description:
      'Turns HDR on the streamed display on or off to match the stream.',
    category: 'hdr',
    group: 'Display',
    control: {
      kind: 'select',
      options: [
        { value: 'auto', label: 'Match the stream' },
        { value: 'disabled', label: 'Don’t change HDR' },
      ],
    },
    default: 'auto',
  },
  {
    key: 'dd_hdr_request_override',
    label: 'HDR request',
    description:
      'Overrides the device’s HDR request for the display, for devices or games that report it wrongly. The stream itself still follows the device.',
    category: 'hdr',
    group: 'Display',
    control: {
      kind: 'select',
      options: [
        { value: 'auto', label: 'Follow the device' },
        { value: 'force_on', label: 'Always on' },
        { value: 'force_off', label: 'Always off' },
      ],
    },
    default: 'auto',
    inactiveHint: "Choose Match the stream under Display HDR to apply the HDR request override.",
    visibleWhen: (values) => text(values, 'dd_hdr_option', 'auto') !== 'disabled',
  },
  {
    key: 'vulkan_hdr_layer',
    label: 'Vulkan HDR on virtual displays',
    description:
      'Registers a Vulkan layer so Vulkan games can use HDR on a virtual display. Turn it off if Vulkan games crash.',
    category: 'hdr',
    group: 'Display',
    control: { kind: 'toggle' },
    default: true,
    advanced: true,
  },
  {
    key: 'hdr_peak_brightness',
    label: 'Peak brightness',
    description: 'The brightest the device’s screen can show, which a virtual display reports as its peak.',
    details: 'A device’s HDR color profile replaces it, unless the device or app sets its own.',
    category: 'hdr',
    group: 'Display',
    control: { kind: 'number', min: 400, max: 2000, step: 1, unit: 'nits' },
    default: 1000,
  },

  // Advanced
  {
    key: 'fallback_mode',
    label: 'Default stream mode',
    description:
      'The resolution and frame rate used when a launch request names none, as WIDTHxHEIGHTxFPS. Devices normally send their own.',
    category: 'advanced',
    group: 'Displays',
    control: { kind: 'text', placeholder: '1920x1080x60', mono: true },
    default: '1920x1080x60',
    advanced: true,
  },
];
