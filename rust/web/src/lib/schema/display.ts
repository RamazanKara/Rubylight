// Settings for the categories named in settings-schema.ts.
import type { ConfigValue } from '../api';
import { nvidiaGate, offersEnhancement, type Setting } from '../settings-types';

type Values = Record<string, ConfigValue>;

/** A text setting as the host reads it: unset or blank means the default. */
function text(values: Values, key: string, fallback: string): string {
  const value = values[key];
  if (value === undefined || value === null) return fallback;
  const trimmed = String(value).trim();
  return trimmed === '' ? fallback : trimmed;
}

/** RTX HDR's settings show where the host offers it. */
const rtxHdr = (values: Values) => offersEnhancement(values, 'rtx_hdr');
const RTX_HDR_HINT =
  'Needs the NVIDIA components (Settings › Picture › Integrations) on an NVIDIA graphics card whose driver offers RTX HDR.';

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
    key: 'output_name',
    label: 'Display',
    description:
      'The monitor that is streamed when streams don’t use a virtual display. Leave it empty to stream the primary display; a device or app can choose its own.',
    details:
      'A stream fails to start when the chosen monitor is unplugged, or is switched off while Monitor setup is not set to turn it on.',
    category: 'display',
    group: 'Streamed display',
    control: { kind: 'display' },
    default: '',
  },
  {
    key: 'virtual_display_mode',
    label: 'Virtual display',
    description:
      'Streams use a virtual display, an extra screen at the device’s resolution and refresh rate, instead of a monitor. Default suits almost everyone; a device’s or app’s choice wins.',
    details:
      'Default (by Windows version) is one for each device on Windows 11 23H2 or later and off on earlier Windows, which cannot load the virtual display driver. One for each device gives every device a display of its own. One shared by all devices is a single display: devices that stream at the same time see the same screen, and a device that asks for another resolution than that display has cannot start its stream. Off streams a monitor: the picture stays visible at the PC, and the monitor’s refresh rate can limit how many new frames the stream gets. A PC with no display switched on streams a virtual display whatever is chosen here.',
    category: 'display',
    group: 'Streamed display',
    control: {
      kind: 'select',
      options: [
        { value: '', label: 'Default (by Windows version)' },
        { value: 'disabled', label: 'Off, stream a monitor' },
        { value: 'per_client', label: 'One for each device' },
        { value: 'shared', label: 'One shared by all devices' },
      ],
    },
    default: '',
    // config.rs virtual_display_mode: per client where os_gate::virtual_display holds.
    defaultText: 'One for each device on Windows 11 23H2 or later, otherwise a monitor',
  },
  {
    key: 'virtual_display_layout',
    label: 'Virtual display layout',
    description:
      'What your monitors do while a stream uses a virtual display. Exclusive turns them off until the stream ends, so games open on the stream; an Extend layout keeps them on.',
    details:
      'Exclusive turns your monitors off while a device streams and back on after it disconnects (see After a stream). Extend keeps your PC display active beside the virtual display. Games open on the primary display: making the virtual display primary moves the main desktop and taskbar to the stream, and keeping the PC display primary leaves them on your monitor. Isolate also places the virtual display far from the other monitors so the mouse cannot wander onto it. A Remote Monitor follows exclusive and primary; choose Extend — keep PC display primary to keep its saved placement.',
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
      'The Windows scale (text and app size) of a virtual display; a fixed percentage Windows does not offer at that resolution stops the stream from starting. Keep leaves it to Windows.',
    details:
      'Keep leaves the scale Windows remembers for that display. Recommended picks one from the resolution, such as 125% at 1080p. A fixed percentage is applied as set, and a stream fails to start where Windows offers no such scale at the display’s resolution. A device that asks for its own scale (PC display scaling in Nightjar) gets that one. This setting scales virtual displays only: a streamed monitor keeps its own scale.',
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
    key: 'dd_activate_virtual_display',
    label: 'Activate a virtual display',
    description:
      'Every stream gets a virtual display, also where Virtual display is Off for the host, a device or an app. Leave it off; Virtual display is the usual way to choose.',
    details:
      'A device that asks for the PC’s own monitor still gets that monitor, and so does a device or app with a display of its own chosen.',
    category: 'display',
    group: 'Streamed display',
    control: { kind: 'toggle' },
    default: false,
    inactiveHint: 'It matters when Virtual display is unset or Off, or a device or app turns it off. Otherwise streams already use a virtual display.',
    // display_session.rs: configured = mode != "disabled" || this, with the mode a device or app may override.
    // Unset is disabled before Windows 11 23H2. Stay visible while on, so a setting that still acts is never hidden.
    visibleWhen: (values) =>
      ['', 'disabled'].includes(text(values, 'virtual_display_mode', '')) || on(values, 'dd_activate_virtual_display', false),
    advanced: true,
  },
  {
    key: 'dd_configuration_option',
    label: 'Monitor setup',
    description:
      'When a stream shows a monitor, not a virtual display, it can turn that monitor on, make it primary, or turn your others off until it ends. The default only checks that it is on.',
    details:
      'Only check that it’s on changes no monitor: a stream of a monitor chosen under Display that is switched off fails to start. Turn it on switches that monitor on beside the others. Turn it on and make it primary also moves the main desktop to it, and Turn it on and the others off turns your other monitors off while the stream runs. Each is undone when the stream ends. All four still set the monitor’s resolution, refresh rate and HDR as those settings say; leaving the monitors alone also skips resolution, refresh rate and HDR changes on it.',
    category: 'display',
    group: 'Streamed display',
    control: {
      kind: 'select',
      options: [
        { value: 'verify_only', label: 'Only check that it’s on' },
        { value: 'ensure_active', label: 'Turn it on' },
        { value: 'ensure_primary', label: 'Turn it on and make it primary' },
        { value: 'ensure_only_display', label: 'Turn it on and the others off' },
        { value: 'disabled', label: 'Leave the monitors alone' },
      ],
    },
    default: 'verify_only',
    cautionWhen: (values) =>
      text(values, 'dd_configuration_option', 'verify_only') === 'ensure_only_display'
        ? 'Your other monitors are off while a stream of a monitor runs.'
        : undefined,
  },
  {
    key: 'dd_resolution_option',
    label: 'Resolution',
    description:
      'The resolution of the streamed display. Match the device suits almost everyone: a streamed monitor switches to the device’s resolution, where it has that mode, and back afterwards.',
    details:
      'A monitor without a mode at the device’s resolution keeps its own, and the picture is scaled to the device. Keeping the current one applies to monitors only; a virtual display matches the device unless a fixed resolution is set. With Monitor setup on Leave the monitors alone, a monitor’s resolution is never changed.',
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
    description:
      'Streams fail to start while this is empty. The streamed display gets this resolution, and the picture is scaled to the size the device asks for.',
    details:
      'A custom resolution is WIDTHxHEIGHT, for example 2560x1440. The width can be 320 to 7680 and the height 200 to 4320. A monitor that has no such mode keeps its own resolution.',
    category: 'display',
    group: 'Resolution and refresh rate',
    control: {
      kind: 'text',
      placeholder: '2560x1440',
      mono: true,
      presets: [
        { value: '1280x720', label: '1280 × 720' },
        { value: '1920x1080', label: '1920 × 1080' },
        { value: '2560x1440', label: '2560 × 1440' },
        { value: '3840x2160', label: '3840 × 2160' },
      ],
    },
    default: '',
    inactiveHint: "Choose Use a fixed resolution under Resolution to apply this value.",
    visibleWhen: (values) => text(values, 'dd_resolution_option', 'auto') === 'manual',
    requiredWhen: (values) => text(values, 'dd_resolution_option', 'auto') === 'manual',
  },
  {
    key: 'dd_refresh_rate_option',
    label: 'Refresh rate',
    description:
      'The refresh rate of the streamed display. Match the stream suits almost everyone: a streamed monitor switches to the stream’s FPS, where it has that rate, and back afterwards.',
    details:
      'Highest available sets a monitor to its fastest rate at that resolution, and Keep the current refresh rate leaves it alone; a monitor slower than the stream’s frame rate cannot supply that many new frames. On a virtual display, matching follows Virtual display refresh under Frame limiting, and the highest and current choices apply to monitors only. With Monitor setup on Leave the monitors alone, a monitor’s refresh rate is never changed.',
    category: 'display',
    group: 'Resolution and refresh rate',
    control: {
      kind: 'select',
      options: [
        { value: 'auto', label: 'Match the stream' },
        { value: 'prefer_highest', label: 'Highest available' },
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
      'Streams fail to start while this is empty. In Hz, such as 59.94; a rate below the stream’s FPS cannot supply every frame, and a virtual display loses its faster automatic refresh.',
    details:
      'In hertz, from 1 to 1000, with up to three decimals such as 59.94 or 119.88. On a virtual display it replaces the refresh that Virtual display refresh sets (twice the stream rate by default), which captures each game frame sooner. A monitor that has no such rate keeps its own.',
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
    label: 'Mode substitutions (JSON)',
    description:
      'Rules that give the streamed display another resolution or refresh rate than a device asks for; a malformed rule stops streams from starting. Leave it empty unless you need one.',
    details:
      'Each entry matches requested_resolution and requested_fps and sets final_resolution and final_refresh_rate. The first matching entry is used. The mixed list is used when resolution and refresh rate both match the device, resolution_only or refresh_rate_only when only that one does. Resolutions are WIDTHxHEIGHT; a result outside 320x200 to 7680x4320 or 1 to 1000 Hz also stops the stream from starting.',
    category: 'display',
    // JSON: three lists of four-field entries, which no form here writes back exactly.
    group: 'Mode substitutions',
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
      'Puts your displays back as they were when a device disconnects, even while its game keeps running. Turn it off only to keep the stream’s display on the desktop for a reconnect.',
    details:
      'Turned off, a disconnected device’s display and layout stay while its app runs, for the time set under Keep a disconnected display for. Exclusive layouts still restore after the delay so the physical monitors become usable again, even with this setting turned off, unless Keep monitors off while paused is on.',
    category: 'display',
    group: 'After a stream',
    control: { kind: 'toggle' },
    default: true,
  },
  {
    key: 'dd_config_revert_delay',
    label: 'Restore delay',
    description:
      'How long the stream’s display and layout are kept after a device disconnects, so a quick reconnect finds them in place. With an exclusive layout your monitors stay off that long.',
    category: 'display',
    group: 'After a stream',
    control: {
      kind: 'number',
      min: 0,
      step: 100,
      unit: 'ms',
      presets: [
        { value: 0, label: 'Right away' },
        { value: 1000, label: '1 second' },
        { value: 3000, label: '3 seconds' },
        { value: 10000, label: '10 seconds' },
        { value: 30000, label: '30 seconds' },
      ],
    },
    default: 3000,
    details: 'Also applies to exclusive layouts when Restore displays on disconnect is turned off, unless Keep monitors off while paused is on. A custom delay is in milliseconds.',
  },
  {
    key: 'dd_paused_virtual_display_timeout_secs',
    label: 'Keep a disconnected display for',
    description:
      'With Restore displays on disconnect off: how long a disconnected device’s display stays on the desktop while its app runs, ready for a reconnect. Then the displays are put back.',
    details: 'Unless Keep monitors off while paused is on, exclusive layouts use the restore delay instead, so the physical monitors become usable again. A custom time is in seconds.',
    category: 'display',
    group: 'After a stream',
    control: {
      kind: 'number',
      min: 0,
      step: 1,
      unit: 's',
      presets: [
        { value: 600, label: '10 minutes' },
        { value: 1800, label: '30 minutes' },
        { value: 7200, label: '2 hours' },
        { value: 28800, label: '8 hours' },
        { value: 0, label: 'Until the app closes' },
      ],
    },
    default: 7200,
    inactiveHint: "Turn off Restore displays on disconnect to keep a disconnected display available for reconnection.",
    visibleWhen: (values) => !on(values, 'dd_config_revert_on_disconnect', true),
  },
  {
    key: 'dd_hold_layout_while_paused',
    label: 'Keep monitors off while paused',
    description:
      'With an exclusive layout, your monitors stay off while a device is away; Win+P cannot bring them back, only a Restore hotkey. Turn it on if a waking monitor pulls the game onto it.',
    details:
      'The monitors stay off until the device returns or its display is no longer kept: after the restore delay, or with Restore displays on disconnect off, after Keep a disconnected display for. For monitors that wake up or re-announce themselves over HDMI and pull the game onto them. While it holds, Win+P cannot bring a monitor back; the restore hotkey still can, once one is set.',
    category: 'display',
    group: 'After a stream',
    control: { kind: 'toggle' },
    default: false,
  },
  {
    key: 'dd_always_restore_from_golden',
    label: 'Restore the saved layout',
    description:
      'After a stream, your monitors return to the layout you saved in Troubleshoot, not the one from before the stream. Turn it on if the earlier layout is not always the one you want.',
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
      'Displays that restoring the saved layout leaves as they are, by their device ID. Leave it empty unless one, such as a dummy plug, must not be touched.',
    category: 'display',
    group: 'After a stream',
    // A JSON list of strings, which a list stores as it is.
    control: { kind: 'list', placeholder: 'Display device ID' },
    default: [],
    advanced: true,
  },
  {
    key: 'dd_snapshot_restore_hotkey',
    label: 'Restore hotkey',
    description:
      'A key on this PC that ends every stream and restores the display layout, for when a stream leaves the screens unusable. F1 to F24, a letter or a digit.',
    details:
      'It is pressed together with the Restore hotkey modifiers, ctrl+alt+shift unless changed. It also removes the virtual displays. A virtual-key code such as 0x2E works too. Empty, 0, none, off or disabled turns it off.',
    category: 'display',
    group: 'After a stream',
    control: { kind: 'text', placeholder: 'Off', mono: true },
    default: '',
  },
  {
    key: 'dd_snapshot_restore_hotkey_modifiers',
    label: 'Restore hotkey modifiers',
    description:
      'Keys held with the restore hotkey: ctrl, alt, shift and win, joined with +. Leave them unless another program uses the combination; none makes the key alone end every stream.',
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
    description:
      'A device used as a remote monitor (an extra screen for this PC) gets picture and input but no audio. Turn it on if such a device should stay silent.',
    category: 'display',
    group: 'Remote monitors',
    control: { kind: 'toggle' },
    default: false,
  },
  {
    key: 'remote_monitor_disconnect_on_stream_end',
    label: 'Remove when its stream ends',
    description:
      'Removes a device’s remote monitor from Windows when its stream ends. Leave it off so the device can resume where it was; turn it on if the leftover display gets in the way.',
    category: 'display',
    group: 'Remote monitors',
    control: { kind: 'toggle' },
    default: false,
  },
  {
    key: 'remote_monitor_disconnect_on_client_disconnect',
    label: 'Remove when the device disconnects',
    description:
      'Removes the remote monitor as soon as the device’s connection drops, even if it was kept for resuming. Leave it off unless a dropped device should not leave its display behind.',
    category: 'display',
    group: 'Remote monitors',
    control: { kind: 'toggle' },
    default: false,
  },
  {
    key: 'remote_monitor_terminate_on_first_request',
    label: 'Quit the game on the first request',
    description:
      'Lets another device quit the running game with its first request. Leave it off: it must then ask again within 60 seconds, and the device that started the game is never asked twice.',
    category: 'library',
    group: 'App behavior',
    control: { kind: 'toggle' },
    default: false,
  },
  {
    key: 'remote_monitor_confirm_app_replacement',
    label: 'Confirm before replacing a running app',
    description:
      'A device must ask twice to replace a running app with a different one, so a game is not closed by accident. Leave it on; turned off, the first request closes the running app.',
    details:
      'The first request to start a different app while one runs is refused, so Moonlight can warn first; starting it again within 60 seconds replaces the app. Remote monitors and remote input still connect without asking.',
    category: 'library',
    group: 'App behavior',
    control: { kind: 'toggle' },
    default: true,
  },

  // Frame limiting
  {
    key: 'frame_limiter_enable',
    label: 'Limit every stream',
    description:
      'Caps games at the stream’s FPS on monitor streams too, for even frame pacing; the game then runs no faster than that on your monitor. Virtual-display streams are capped already.',
    details:
      'Virtual-display streams are already limited unless Virtual display refresh is off. The cap is the Frame limit, or the stream’s frame rate. It needs RTSS (see Limiter): without it the stream says the frame limit was not applied.',
    category: 'frame-limiting',
    group: 'Frame limiter',
    control: { kind: 'toggle' },
    default: false,
  },
  {
    key: 'frame_limiter_provider',
    label: 'Limiter',
    description:
      'What caps the game’s FPS. Automatic and RTSS both use RivaTuner Statistics Server (RTSS), which must be installed; leave it on Automatic, or choose None to turn all limiting off.',
    details:
      'On a PC without RTSS, Automatic leaves the automatic limit on virtual displays off without a warning, while RTSS, or Limit every stream, says on the stream that the limit was not applied. Rubylight starts RTSS when a limit is needed and puts its previous limit back after the stream. None also turns off the automatic limit on virtual displays.',
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
      'The FPS games are capped at while streaming; 0 uses the stream’s frame rate. Leave it at 0 unless games should run at another rate; RTSS keeps decimals such as 59.94.',
    category: 'frame-limiting',
    group: 'Frame limiter',
    control: { kind: 'number', min: 0, max: 1000, step: 0.001, unit: 'fps' },
    default: 0,
  },
  {
    key: 'frame_limiter_auto_virtual_framegen',
    label: 'Virtual display refresh',
    description:
      'A virtual display’s refresh rate: a faster one captures each game frame sooner after it is drawn. Twice the stream rate suits almost everyone; Off also ends the automatic FPS cap.',
    details:
      'Every choice but Off also caps games at the frame limit and uses 1000 Hz when the device asks for VRR. Off runs the display at the stream’s frame rate. On an RX 7900 XT, four times the stream rate measured within 0.15 ms of twice, and 1000 Hz was no faster at 120 fps and about 1 ms slower at 60 fps (14.28 against 13.15 ms from render to received).',
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
    description:
      'Where RivaTuner Statistics Server (RTSS) is installed. Leave it empty, which looks in Program Files, unless RTSS is installed elsewhere and is not found.',
    category: 'frame-limiting',
    group: 'RTSS',
    control: { kind: 'text', placeholder: 'C:\\Program Files (x86)\\RivaTuner Statistics Server', mono: true, browse: 'directory' },
    default: '',
    inactiveHint: "Choose Automatic or RTSS under Limiter to use RTSS settings. RTSS must be installed on this PC.",
    visibleWhen: rtss,
  },
  {
    key: 'rtss_frame_limit_type',
    label: 'RTSS limit mode',
    description:
      'How RTSS times frames at the FPS cap (its limiter mode). Leave it on Asynchronous unless you know these modes; virtual-display streams and frame-generation games choose their own.',
    details:
      'Virtual-display streams, unless Virtual display refresh is off, and frame-generation games on a monitor use NVIDIA Reflex when this PC streams with an NVIDIA GPU, and front edge sync otherwise. NVIDIA Reflex needs an NVIDIA GPU; on other GPUs it limits with front edge sync.',
    category: 'frame-limiting',
    group: 'RTSS',
    control: {
      kind: 'select',
      options: [
        { value: 'async', label: 'Asynchronous' },
        { value: 'front edge sync', label: 'Front edge sync' },
        { value: 'back edge sync', label: 'Back edge sync' },
        { value: 'nvidia reflex', label: 'NVIDIA Reflex' },
      ],
    },
    // framegen.rs reads letters and digits only, so the fallback console's front-edge-sync is front edge sync.
    aliases: { match: 'letters' },
    default: 'async',
    inactiveHint: "Choose Automatic or RTSS under Limiter to use RTSS settings. RTSS must be installed on this PC.",
    visibleWhen: rtss,
  },
  {
    key: 'rtss_allow_virtual_display_override',
    label: 'Use this mode on virtual displays',
    description:
      'Applies the RTSS limit mode above to virtual-display streams, in place of the mode they choose themselves. Leave it off unless you want to try another RTSS mode there.',
    details:
      'Those streams otherwise use NVIDIA Reflex when this PC streams with an NVIDIA GPU, and front edge sync on other GPUs. Apps set to game-provided frame generation keep that mode, because other modes can add a lot of latency there.',
    category: 'frame-limiting',
    group: 'RTSS',
    control: { kind: 'toggle' },
    default: false,
    inactiveHint: "Choose Automatic or RTSS under Limiter to use RTSS settings. RTSS must be installed on this PC.",
    visibleWhen: rtss,
    advanced: true,
  },

  // HDR
  {
    key: 'dd_hdr_option',
    label: 'Display HDR',
    description:
      'Match the stream turns HDR on the streamed display on for an HDR stream, off for an SDR one, and back afterwards. Don’t change HDR can leave an HDR stream without HDR highlights.',
    details:
      'A streamed monitor’s HDR is switched too, unless Monitor setup is on Leave the monitors alone. While RTX HDR converts the picture, the streamed display stays in SDR.',
    category: 'hdr',
    group: 'HDR',
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
      'Forces HDR on the streamed display on or off, for devices or games that report it wrongly; the stream itself still follows the device. Leave it on Follow the device otherwise.',
    category: 'hdr',
    group: 'HDR',
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
      'Lets Vulkan games use HDR on a virtual display, by registering a Vulkan layer (an add-on Vulkan games load) on this PC. Leave it on; turn it off only if Vulkan games crash.',
    category: 'hdr',
    group: 'HDR',
    control: { kind: 'toggle' },
    default: true,
    advanced: true,
  },
  {
    key: 'virtual_display_hdr_calibration',
    label: 'Windows HDR Calibration on virtual displays',
    description:
      'Gives an HDR virtual display the profile Windows HDR Calibration made for your HDR monitor. Leave it off: that profile describes your monitor, not the device you stream to.',
    details:
      'The display gets its own profile back after the stream. A profile chosen for the device comes first. Where no HDR monitor has a calibration profile, the stream stays uncalibrated and the host log says so.',
    category: 'hdr',
    group: 'HDR',
    control: { kind: 'toggle' },
    default: false,
    advanced: true,
  },
  {
    key: 'hdr_peak_brightness',
    label: 'Peak brightness',
    description:
      'The HDR peak brightness, in nits, that a virtual display reports as its own. Set it to the brightest the device’s screen can show.',
    details: 'A device’s HDR color profile replaces it, unless the device or app sets its own. RTX HDR converts to this peak too.',
    category: 'hdr',
    group: 'HDR',
    control: { kind: 'number', min: 400, max: 2000, step: 1, unit: 'nits' },
    default: 1000,
  },
  {
    key: 'rtx_hdr',
    label: 'RTX HDR',
    description:
      'Turns a game’s SDR picture into HDR, adding GPU work to every frame. It runs only where an app or device turns it on; this switch alone converts nothing.',
    details:
      'It converts only for a device that asks for HDR. While RTX HDR converts the picture, the streamed display stays in SDR and Prefer 10-bit SDR does not apply. It uses the peak brightness above.',
    category: 'hdr',
    group: 'RTX HDR',
    control: { kind: 'toggle' },
    default: false,
    visibleWhen: rtxHdr,
    hostGated: nvidiaGate,
    inactiveHint: RTX_HDR_HINT,
  },
  {
    key: 'rtx_hdr_sdr_brightness',
    label: 'SDR brightness',
    description:
      'Raises the brightness of the SDR picture in an RTX HDR stream, from 0 to 100; 0 leaves it unchanged. Raise it if the converted picture looks too dark on the device.',
    details: 'Each step adds 1 nit to the SDR white of 100 nits, up to 200 nits at 100.',
    category: 'hdr',
    group: 'RTX HDR',
    control: { kind: 'number', min: 0, max: 100, step: 1 },
    default: 0,
    visibleWhen: rtxHdr,
    hostGated: nvidiaGate,
    inactiveHint: RTX_HDR_HINT,
  },
  {
    key: 'rtx_hdr_contrast',
    label: 'Contrast',
    description:
      'Contrast of the RTX HDR picture, from −100 to 100; 0 is neutral. A value in the game’s NVIDIA driver profile wins, and the desktop always uses 0.',
    category: 'hdr',
    group: 'RTX HDR',
    control: { kind: 'number', min: -100, max: 100, step: 1 },
    default: 0,
    visibleWhen: rtxHdr,
    hostGated: nvidiaGate,
    inactiveHint: RTX_HDR_HINT,
  },
  {
    key: 'rtx_hdr_saturation',
    label: 'Saturation',
    description:
      'Color saturation of the RTX HDR picture, from −100 to 100; 0 is neutral. A value in the game’s NVIDIA driver profile wins, and the desktop always uses 0.',
    category: 'hdr',
    group: 'RTX HDR',
    control: { kind: 'number', min: -100, max: 100, step: 1 },
    default: 0,
    visibleWhen: rtxHdr,
    hostGated: nvidiaGate,
    inactiveHint: RTX_HDR_HINT,
  },
  {
    key: 'rtx_hdr_middle_gray',
    label: 'Middle gray',
    description:
      'Mid-tone brightness of the RTX HDR picture, from 10 to 100; higher is brighter. A value in the game’s NVIDIA driver profile wins, and the desktop always uses 50.',
    category: 'hdr',
    group: 'RTX HDR',
    control: { kind: 'number', min: 10, max: 100, step: 1 },
    default: 50,
    visibleWhen: rtxHdr,
    hostGated: nvidiaGate,
    inactiveHint: RTX_HDR_HINT,
  },

  // Used when a launch names no mode.
  {
    key: 'fallback_mode',
    label: 'Default stream mode',
    description:
      'The resolution and FPS a stream gets when the device names none. Devices normally send their own, so leave it as it is.',
    details: 'A custom mode is WIDTHxHEIGHTxFPS, for example 1920x1080x60.',
    category: 'display',
    group: 'Resolution and refresh rate',
    control: {
      kind: 'text',
      placeholder: '1920x1080x60',
      mono: true,
      presets: [
        { value: '1280x720x60', label: '1280 × 720 at 60 fps' },
        { value: '1920x1080x60', label: '1920 × 1080 at 60 fps' },
        { value: '1920x1080x120', label: '1920 × 1080 at 120 fps' },
        { value: '2560x1440x60', label: '2560 × 1440 at 60 fps' },
        { value: '2560x1440x120', label: '2560 × 1440 at 120 fps' },
        { value: '3840x2160x60', label: '3840 × 2160 at 60 fps' },
      ],
    },
    default: '1920x1080x60',
    advanced: true,
  },
];
