// Settings for the general, network, audio, input and commands categories.
// Defaults are the ones the Rust host falls back to when a key is unset.
import type { ConfigValue } from '../api';
import type { Option, Setting } from '../settings-types';

type Values = Record<string, ConfigValue>;

/** A boolean read as the host reads it; anything unrecognised keeps the default. */
function on(values: Values, key: string, fallback: boolean): boolean {
  const value = values[key];
  if (typeof value === 'boolean') return value;
  const word = String(value ?? '')
    .trim()
    .replace(/^"(.*)"$/, '$1')
    .toLowerCase();
  if (['true', 'yes', '1', 'enable', 'enabled', 'on'].includes(word)) return true;
  if (['false', 'no', '0', 'disable', 'disabled', 'off'].includes(word)) return false;
  return fallback;
}

/** A text setting read as the host reads it: blank means the default. */
function text(values: Values, key: string, fallback: string): string {
  const value = values[key];
  const raw = value === null || value === undefined ? '' : String(value);
  return raw.trim() === '' ? fallback : raw;
}

const locales: Option[] = [
  { value: 'en', label: 'English' },
  { value: 'en_GB', label: 'English (UK)' },
  { value: 'en_US', label: 'English (US)' },
  { value: 'bg', label: 'Български' },
  { value: 'cs', label: 'Čeština' },
  { value: 'de', label: 'Deutsch' },
  { value: 'es', label: 'Español' },
  { value: 'fr', label: 'Français' },
  { value: 'hu', label: 'Magyar' },
  { value: 'it', label: 'Italiano' },
  { value: 'ja', label: '日本語' },
  { value: 'ko', label: '한국어' },
  { value: 'pl', label: 'Polski' },
  { value: 'pt', label: 'Português' },
  { value: 'pt_BR', label: 'Português (Brasil)' },
  { value: 'ru', label: 'Русский' },
  { value: 'sv', label: 'Svenska' },
  { value: 'tr', label: 'Türkçe' },
  { value: 'uk', label: 'Українська' },
  { value: 'vi', label: 'Tiếng Việt' },
  { value: 'zh', label: '中文（简体）' },
  { value: 'zh_TW', label: '中文（繁體）' },
];

const general: Setting[] = [
  {
    key: 'sunshine_name',
    label: 'Host name',
    description:
      "The name Moonlight shows for this host. Leave it empty to use the PC's name; local network discovery picks up a change after a restart.",
    category: 'general',
    group: 'Host',
    control: { kind: 'text', placeholder: "This PC's name" },
    default: '',
  },
  {
    key: 'locale',
    label: 'Fallback console language',
    description: 'Language used by the fallback console and reported to connected tools. This console is currently available in English only.',
    category: 'general',
    group: 'Host',
    control: { kind: 'select', options: locales },
    // console/i18n.rs locale: any case, - for _.
    aliases: { match: 'dash' },
    default: 'en',
  },
  {
    key: 'enable_pairing',
    label: 'Allow pairing',
    description:
      'Let new devices pair with this host. Turn it off once your devices are paired; devices already paired keep working.',
    category: 'general',
    group: 'Host',
    control: { kind: 'toggle' },
    default: true,
  },
  {
    key: 'system_tray',
    label: 'Tray icon',
    description: 'Show a Rubylight icon in the Windows notification area, with a shortcut to this console.',
    category: 'general',
    group: 'Tray',
    control: { kind: 'toggle' },
    default: true,
    restart: true,
  },
  {
    key: 'hide_tray_controls',
    label: 'Hide tray controls',
    description:
      "Remove the disconnect, restart and quit actions from the tray icon's menu, so people at the PC cannot stop the host from there.",
    category: 'general',
    group: 'Tray',
    control: { kind: 'toggle' },
    default: false,
    inactiveHint: "Turn on Tray icon to use this setting.",
    visibleWhen: (values) => on(values, 'system_tray', true),
    restart: true,
  },
  {
    key: 'update_check_interval',
    label: 'Update check interval',
    description:
      'How often the host looks for a new Rubylight release. Set it to 0 to turn off automatic checks.',
    details:
      'The minimum is 3600 seconds (one hour). 0 also turns off automatic installation; you can still check and install from Maintenance.',
    category: 'general',
    group: 'Updates',
    control: { kind: 'number', min: 0, step: 1, unit: 's' },
    default: 86400,
  },
  {
    key: 'auto_update',
    label: 'Install updates automatically',
    description: 'Download and install new releases on their own while nothing is streaming.',
    details: 'Only verified releases are downloaded. They install after one minute without streams, pending connections or running host apps. Requires the Windows service and an update check interval above 0. Off by default.',
    category: 'general',
    group: 'Updates',
    control: { kind: 'toggle' },
    default: false,
  },
  {
    key: 'notify_pre_releases',
    label: 'Include pre-releases',
    description: 'Also offer pre-release versions when checking for updates. They get fixes sooner but may have more bugs.',
    category: 'general',
    group: 'Updates',
    control: { kind: 'toggle' },
    default: false,
  },
  {
    key: 'min_log_level',
    label: 'Log level',
    description:
      'How much detail the host writes to its log; changes apply right away. Use Debug when you are preparing a problem report.',
    details:
      'Information includes encoder settings, display setup and timing lines every 5 seconds. Debug adds GPU scheduling, discovery and probe details; Verbose adds everything.',
    category: 'general',
    group: 'Logging',
    control: {
      kind: 'select',
      options: [
        { value: 'verbose', label: 'Verbose' },
        { value: 'debug', label: 'Debug' },
        { value: 'info', label: 'Information' },
        { value: 'warning', label: 'Warning' },
        { value: 'error', label: 'Error' },
        { value: 'none', label: 'None' },
      ],
    },
    // config.rs log_level, and the numbers the fallback console writes.
    aliases: {
      match: 'case',
      spellings: {
        '0': 'verbose',
        trace: 'verbose',
        '1': 'debug',
        '2': 'info',
        '3': 'warning',
        warn: 'warning',
        '4': 'error',
        '5': 'error',
        fatal: 'error',
        '6': 'none',
        off: 'none',
      },
    },
    default: 'info',
  },
  {
    key: 'hardware_metrics',
    label: 'Send hardware metrics to client',
    description: 'Share supported CPU and GPU readings when a compatible client requests them during a stream. Off by default; unavailable readings are omitted.',
    category: 'general',
    group: 'Hardware metrics',
    control: { kind: 'toggle' },
    default: false,
  },
  {
    key: 'hardware_metrics_interval',
    label: 'Hardware metrics interval',
    description: 'Minimum time between samples requested by a streaming client, from 500 to 10000 milliseconds.',
    category: 'general',
    group: 'Hardware metrics',
    control: { kind: 'number', min: 500, max: 10000, step: 1, unit: 'ms' },
    default: 1000,
    inactiveHint: "Turn on Send hardware metrics to client to use this setting.",
    visibleWhen: (values) => on(values, 'hardware_metrics', false),
  },
  {
    key: 'hardware_metrics_selection',
    label: 'Hardware metrics selection',
    description: 'Which readings to send. Leave it empty to send every supported reading.',
    details: 'To send only some readings, add their identifiers: cpu_usage_percent, gpu_usage_percent, gpu_clock_mhz, gpu_memory_clock_mhz, gpu_temperature_c, gpu_hotspot_temperature_c, gpu_memory_temperature_c, gpu_voltage_mv, gpu_fan_rpm, gpu_fan_percent, gpu_power_w, gpu_board_power_w, gpu_vram_used_mb, gpu_vram_usage_percent.',
    category: 'general',
    group: 'Hardware metrics',
    control: { kind: 'list', placeholder: 'cpu_usage_percent' },
    default: [],
    inactiveHint: "Turn on Send hardware metrics to client to use this setting.",
    visibleWhen: (values) => on(values, 'hardware_metrics', false),
  },
  {
    key: 'log_path',
    label: 'Log file',
    description: "Where the host writes its log. A relative path is inside the host's configuration folder.",
    category: 'general',
    group: 'Logging',
    control: { kind: 'text', placeholder: 'logs/butterpollo.log', mono: true },
    default: 'logs/butterpollo.log',
    restart: true,
  },
  {
    key: 'legacy_ordering',
    label: 'App order for older devices',
    description:
      'Add invisible characters to app names so older versions of Moonlight list apps in your order. This can break scripts and tools that match app names exactly.',
    category: 'general',
    group: 'Compatibility',
    control: { kind: 'toggle' },
    default: false,
  },
  {
    key: 'envvar_compatibility_mode',
    label: 'Whole-number frame rate for commands',
    description:
      'Give commands SUNSHINE_CLIENT_FPS as a whole number, such as 60 instead of 59.940. Turn it on if a script cannot read fractional frame rates.',
    category: 'general',
    group: 'Compatibility',
    control: { kind: 'toggle' },
    default: false,
  },
];

const encryption: Option[] = [
  { value: '0', label: 'Off' },
  { value: '1', label: 'When the device supports it' },
  { value: '2', label: 'Required' },
];

const network: Setting[] = [
  {
    key: 'port',
    label: 'Base port',
    description:
      "Moonlight connects on this port, and the host's other ports are counted from it. Change it only if another program needs these ports.",
    details:
      'Moonlight also connects on the port 5 below it, this console uses the next port up, and streams use ports 9, 10, 11 and 21 above it.',
    category: 'network',
    group: 'Ports and addresses',
    control: { kind: 'number', min: 1029, max: 65514, step: 1 },
    default: 47989,
    restart: true,
  },
  {
    key: 'address_family',
    label: 'IP versions',
    description: 'Whether the host also accepts connections over IPv6. Ignored when a listen address is set.',
    category: 'network',
    group: 'Ports and addresses',
    control: {
      kind: 'select',
      options: [
        { value: 'ipv4', label: 'IPv4 only' },
        { value: 'both', label: 'IPv4 and IPv6' },
      ],
    },
    default: 'ipv4',
    restart: true,
  },
  {
    key: 'bind_address',
    label: 'Listen address',
    description:
      "Listen on one of this PC's IP addresses instead of all of them; local network discovery then uses only that address. Leave it empty to listen on every address.",
    category: 'network',
    group: 'Ports and addresses',
    control: { kind: 'text', placeholder: 'All addresses', mono: true },
    default: '',
    restart: true,
  },
  {
    key: 'enable_discovery',
    label: 'Local network discovery',
    description: 'Announce this host on the local network so Moonlight finds it without typing an address.',
    category: 'network',
    group: 'Discovery and internet',
    control: { kind: 'toggle' },
    default: true,
    restart: true,
  },
  {
    key: 'upnp',
    label: 'UPnP port forwarding',
    description:
      'Ask the router to forward the streaming ports so devices can connect over the internet. The router must have Universal Plug and Play (UPnP) turned on.',
    category: 'network',
    group: 'Discovery and internet',
    control: { kind: 'toggle' },
    default: false,
    restart: true,
  },
  {
    key: 'webrtc_server',
    label: 'WebRTC server (Experimental)',
    description: 'Let TVs and browsers that speak the Moonlight WebRTC protocol pair and sign in. Streaming over WebRTC is not available yet.',
    details:
      'Pairing requests appear on the PIN page. Signalling is plain ws:// on the local network: signing in keeps other devices from using this PC, but does not hide the traffic. Only devices on the local network are accepted.',
    category: 'network',
    group: 'WebRTC',
    control: { kind: 'toggle' },
    default: false,
    restart: true,
  },
  {
    key: 'webrtc_port',
    label: 'WebRTC signalling port',
    description: 'The TCP port of the WebRTC server and the UDP port TVs send their discovery broadcast to.',
    details: 'If something else uses the port, the console shows a warning and the server does not start. The port is never changed for you.',
    category: 'network',
    group: 'WebRTC',
    control: { kind: 'number', min: 1024, max: 65535, step: 1 },
    default: 8000,
    restart: true,
    visibleWhen: (values) => on(values, 'webrtc_server', false),
  },
  {
    key: 'webrtc_media_port',
    label: 'WebRTC media port',
    description: 'The UDP port for WebRTC video, audio and input once streaming over WebRTC is available.',
    category: 'network',
    group: 'WebRTC',
    control: { kind: 'number', min: 1024, max: 65535, step: 1 },
    default: 8001,
    restart: true,
    visibleWhen: (values) => on(values, 'webrtc_server', false),
  },
  {
    key: 'lan_encryption_mode',
    label: 'Local network encryption',
    description:
      'Whether streams to devices on the local network, including Tailscale, are encrypted. Encryption costs some performance on slower devices.',
    details: 'Required turns away devices that cannot encrypt.',
    category: 'network',
    group: 'Streams',
    control: { kind: 'select', options: encryption },
    default: 0,
  },
  {
    key: 'wan_encryption_mode',
    label: 'Internet encryption',
    description:
      'Whether streams to devices outside the local network are encrypted. Required turns away devices that cannot encrypt.',
    category: 'network',
    group: 'Streams',
    control: { kind: 'select', options: encryption },
    default: 1,
  },
  {
    key: 'ping_timeout',
    label: 'Device timeout',
    description:
      'How long the host waits without hearing from a device before it ends the stream. Raise it if streams drop on an unreliable network.',
    category: 'network',
    group: 'Streams',
    control: { kind: 'number', min: 1000, max: 300000, step: 1, unit: 'ms' },
    default: 10000,
  },
  {
    key: 'origin_web_ui_allowed',
    label: 'Console access',
    description:
      'Which addresses may open this console and use its API.',
    details:
      'Local network includes private addresses and Tailscale (100.64.0.0/10). With Any network and UPnP on, the console port is forwarded too.',
    category: 'network',
    group: 'Console',
    control: {
      kind: 'select',
      options: [
        { value: 'pc', label: 'This PC only' },
        { value: 'lan', label: 'Local network' },
        { value: 'wan', label: 'Any network' },
      ],
    },
    default: 'lan',
  },
  {
    key: 'csrf_allowed_origins',
    label: 'Trusted origins',
    description:
      'Other web addresses allowed to make changes through this console. Add one only when you reach the console through a reverse proxy.',
    details: 'Write each one as scheme and host, such as https://stream.example.com.',
    category: 'network',
    group: 'Console',
    control: { kind: 'list', placeholder: 'https://stream.example.com' },
    default: [],
  },
  {
    key: 'session_token_ttl_seconds',
    label: 'Sign-in token lifetime',
    description:
      'How long a console sign-in lasts before the browser must renew it.',
    details:
      'From 60 to 604800 seconds. Without "Keep me signed in", a sign-in ends after this time or one day, whichever is longer.',
    category: 'network',
    group: 'Console',
    control: { kind: 'number', min: 60, max: 604800, step: 1, unit: 's' },
    default: 7200,
  },
  {
    key: 'remember_me_refresh_token_ttl_seconds',
    label: 'Remembered sign-in lifetime',
    description:
      'How long a browser signed in with "Keep me signed in" stays signed in, up to 366 days. Lower it on shared computers.',
    category: 'network',
    group: 'Console',
    control: { kind: 'number', min: 60, max: 31622400, step: 1, unit: 's' },
    default: 604800,
  },
];

const streamsAudio = (values: Values) => on(values, 'stream_audio', true);
const systemAudio = (values: Values) => streamsAudio(values) && text(values, 'audio_capture_mode', 'system') === 'system';

const audio: Setting[] = [
  {
    key: 'stream_audio',
    label: 'Stream audio',
    description:
      "Send the host's sound to the device. Turn it off when a stream is only used as an extra monitor and should stay silent.",
    category: 'audio',
    group: 'Sound',
    control: { kind: 'toggle' },
    default: true,
  },
  {
    key: 'audio_capture_mode',
    label: 'Audio to stream',
    description:
      'Choose Game only to stream just the game, keeping browsers and notifications out. Reconnect to apply.',
    details:
      'Game only captures the launched game and its children. It requires a verified game process; when capture is unavailable, the stream sends silence with a session warning. Game only currently uses stereo and leaves local playback devices unchanged.',
    category: 'audio',
    group: 'Sound',
    control: { kind: 'select', options: [
      { value: 'system', label: 'System audio (selected playback device)' },
      { value: 'game', label: 'Game only' },
    ] },
    default: 'system',
    inactiveHint: "Turn on Stream audio to use this setting.",
    visibleWhen: streamsAudio,
  },
  {
    key: 'audio_sink',
    label: 'Audio device',
    description:
      'The audio output captured when the streaming device keeps sound playing on the host, or when no virtual speakers exist. Leave it empty to use the Windows default output.',
    category: 'audio',
    group: 'Devices',
    control: { kind: 'audio' },
    default: '',
    inactiveHint: "Turn on Stream audio and choose System audio under Audio to stream to use this setting.",
    visibleWhen: systemAudio,
  },
  {
    key: 'virtual_sink',
    label: 'Virtual speakers',
    description:
      "The audio output made the Windows default during a stream, so sound goes to the streaming device instead of the PC's speakers.",
    details:
      'Leave it empty to use Steam Streaming Speakers when installed. An output chosen here is used even when the streaming device keeps sound on the host.',
    category: 'audio',
    group: 'Devices',
    control: { kind: 'audio' },
    default: '',
    inactiveHint: "Turn on Stream audio and choose System audio under Audio to stream to use this setting.",
    visibleWhen: systemAudio,
  },
  {
    key: 'audio_sink_capture_only',
    label: 'Capture without switching',
    description:
      'Capture the audio device without changing the Windows default output. You then send the apps you want to hear to that device yourself.',
    details: 'Applies only when an audio device is set and virtual speakers are not.',
    category: 'audio',
    group: 'Devices',
    control: { kind: 'toggle' },
    default: false,
    inactiveHint: "Choose System audio, set an Audio device and leave Virtual speakers empty to use this setting.",
    visibleWhen: (values) =>
      systemAudio(values) && text(values, 'audio_sink', '') !== '' && text(values, 'virtual_sink', '') === '',
  },
  {
    key: 'auto_capture_sink',
    label: 'Follow the default device',
    description:
      'Capture whichever device is the Windows default, even if it changes during a stream. Turn it off to stay on the device chosen when the stream started.',
    details: 'Applies only without virtual speakers. When on, capture is also retried when it fails.',
    category: 'audio',
    group: 'Devices',
    control: { kind: 'toggle' },
    default: true,
    inactiveHint: "Turn on Stream audio and choose System audio under Audio to stream to use this setting.",
    visibleWhen: systemAudio,
  },
  {
    key: 'keep_sink_default',
    label: 'Keep virtual speakers as default',
    description:
      'If another device becomes the Windows default during a stream, switch back to the virtual speakers. The device you picked is still restored when the stream ends.',
    category: 'audio',
    group: 'Virtual speakers',
    control: { kind: 'toggle' },
    default: true,
    inactiveHint: "Turn on Stream audio and choose System audio under Audio to stream to use this setting.",
    visibleWhen: systemAudio,
  },
  {
    key: 'install_steam_audio_drivers',
    label: 'Install Steam audio drivers',
    description:
      'When Steam is installed, install its streaming speakers and microphone when a stream needs them.',
    details:
      'Steam Streaming Speakers are installed at the start of a stream if no virtual speakers exist, and Steam Streaming Microphone the first time a device sends its microphone. The speakers keep the PC silent and support 5.1 and 7.1 surround.',
    category: 'audio',
    group: 'Virtual speakers',
    control: { kind: 'toggle' },
    default: true,
    inactiveHint: "Turn on Stream audio or Use the device microphone to install the corresponding Steam audio driver when needed.",
    visibleWhen: (values) => streamsAudio(values) || on(values, 'stream_mic', true),
  },
  {
    key: 'stream_mic',
    label: 'Use the device microphone',
    description:
      'Let a streaming device that sends its microphone talk through the PC. Only clients with microphone support send one.',
    details: 'While forwarding, Steam Streaming Microphone becomes the Windows default input. Your previous microphones return when forwarding stops. Apps with a fixed input must select Default or Steam Streaming Microphone.',
    category: 'audio',
    group: 'Microphone',
    control: { kind: 'toggle' },
    default: true,
  },
];

const keyboardOn = (values: Values) => on(values, 'keyboard', true);
const mouseOn = (values: Values) => on(values, 'mouse', true);
const controllerOn = (values: Values) => on(values, 'controller', true);

/** What a back grip can press on the virtual controller, which has none. */
const gripOptions: Option[] = [
  { value: 'none', label: 'Nothing' },
  { value: 'a', label: 'A (Cross)' },
  { value: 'b', label: 'B (Circle)' },
  { value: 'x', label: 'X (Square)' },
  { value: 'y', label: 'Y (Triangle)' },
  { value: 'lb', label: 'Left bumper (L1)' },
  { value: 'rb', label: 'Right bumper (R1)' },
  { value: 'lt', label: 'Left trigger (L2), fully' },
  { value: 'rt', label: 'Right trigger (R2), fully' },
  { value: 'l3', label: 'Left stick click (L3)' },
  { value: 'r3', label: 'Right stick click (R3)' },
  { value: 'back', label: 'View / Back (Create, Share)' },
  { value: 'start', label: 'Menu / Start (Options)' },
  { value: 'guide', label: 'Guide (PS)' },
  { value: 'dpad_up', label: 'D-pad up' },
  { value: 'dpad_down', label: 'D-pad down' },
  { value: 'dpad_left', label: 'D-pad left' },
  { value: 'dpad_right', label: 'D-pad right' },
  { value: 'touchpad', label: 'Touchpad click' },
  { value: 'misc', label: 'Share (Xbox) / Mute (DualSense)' },
];

/** One back grip setting; Moonlight sends the grips as paddles 1 to 4. */
function backGrip(key: string, name: string, where: string): Setting {
  return {
    key,
    label: `Back grip ${name}`,
    description: `What the ${where} back button presses: a Steam Deck's ${name}, or the matching paddle on an Xbox Elite or DualSense Edge.`,
    details: `The virtual controllers have no back buttons, so they do nothing until mapped here. A Steam Deck passed through as a Steam Deck keeps its own.`,
    category: 'input',
    group: 'Steam Deck and back grips',
    control: { kind: 'select', options: gripOptions },
    default: 'none',
    inactiveHint: "Turn on Controllers under Input to use this setting. Paired devices also need the Controllers permission.",
    visibleWhen: controllerOn,
  };
}

const input: Setting[] = [
  {
    key: 'keyboard',
    label: 'Keyboard',
    description: 'Let devices type on the host.',
    category: 'input',
    group: 'Devices',
    control: { kind: 'toggle' },
    default: true,
  },
  {
    key: 'mouse',
    label: 'Mouse',
    description: 'Let devices use the mouse on the host. Pen and touch input need this too.',
    category: 'input',
    group: 'Devices',
    control: { kind: 'toggle' },
    default: true,
  },
  {
    key: 'controller',
    label: 'Controllers',
    description: 'Let devices use game controllers on the host.',
    category: 'input',
    group: 'Devices',
    control: { kind: 'toggle' },
    default: true,
  },
  {
    key: 'enable_input_only_mode',
    label: 'Remote input entry',
    description:
      "Add a Remote Input entry to every device's app list. It sends keyboard, mouse and controller input to the host without video, for example from a second device.",
    category: 'input',
    group: 'Devices',
    control: { kind: 'toggle' },
    default: false,
  },
  {
    key: 'always_send_scancodes',
    label: 'Send scan codes',
    description:
      'Send keys as scan codes, which some games need to see keyboard input at all. Turn it off if a device with a non-US layout types the wrong characters.',
    category: 'input',
    group: 'Keyboard',
    control: { kind: 'toggle' },
    default: true,
    inactiveHint: "Turn on Keyboard under Input to use this setting. Paired devices also need the Keyboard permission.",
    visibleWhen: keyboardOn,
  },
  {
    key: 'key_rightalt_to_key_win',
    label: 'Right Alt as Windows key',
    description: 'Treat the right Alt key as the Windows key, for devices that cannot send the Windows key.',
    category: 'input',
    group: 'Keyboard',
    control: { kind: 'toggle' },
    default: false,
    inactiveHint: "Turn on Keyboard under Input to use this setting. Paired devices also need the Keyboard permission.",
    visibleWhen: keyboardOn,
  },
  {
    key: 'key_repeat_delay',
    label: 'Key repeat delay',
    description: 'How long a held key waits before it starts repeating. 0 turns key repeat off on the host.',
    category: 'input',
    group: 'Keyboard',
    control: { kind: 'number', min: 0, max: 60000, step: 1, unit: 'ms' },
    default: 500,
    inactiveHint: "Turn on Keyboard under Input to use this setting. Paired devices also need the Keyboard permission.",
    visibleWhen: keyboardOn,
  },
  {
    key: 'key_repeat_frequency',
    label: 'Key repeat rate',
    description: 'How many times per second a held key repeats. Decimals are allowed; 0 uses the default.',
    category: 'input',
    group: 'Keyboard',
    control: { kind: 'number', min: 0, max: 1000, step: 0.1, unit: 'Hz' },
    default: 24.9,
    inactiveHint: "Turn on Keyboard under Input to use this setting. Paired devices also need the Keyboard permission.",
    visibleWhen: keyboardOn,
  },
  {
    key: 'keybindings',
    label: 'Key remapping',
    description:
      'Make a key from the device press a different key on the host, as pairs of Windows virtual-key codes such as ["0x10", "0xA0"].',
    details:
      'All pairs go in one list, each key from the device followed by the key the host presses instead. Shift, Ctrl and Alt already map to their left-hand keys.',
    category: 'input',
    group: 'Keyboard',
    control: { kind: 'json' },
    default: [],
    inactiveHint: "Turn on Keyboard under Input to use this setting. Paired devices also need the Keyboard permission.",
    visibleWhen: keyboardOn,
  },
  {
    key: 'high_resolution_scrolling',
    label: 'High-resolution scrolling',
    description:
      'Pass fine scroll steps from the device. Turn it off for older apps that scroll too far, so scrolling moves in whole notches.',
    category: 'input',
    group: 'Mouse, pen and touch',
    control: { kind: 'toggle' },
    default: true,
    inactiveHint: "Turn on Mouse under Input to use this setting. Paired devices also need the Mouse permission.",
    visibleWhen: mouseOn,
  },
  {
    key: 'native_pen_touch',
    label: 'Native pen and touch',
    description:
      'Pass pen and touch from devices to Windows as pen and touch. Turn it off for apps that handle them badly; devices then send touch as mouse input.',
    category: 'input',
    group: 'Mouse, pen and touch',
    control: { kind: 'toggle' },
    default: true,
    inactiveHint: "Turn on Mouse under Input to use this setting. Paired devices also need the Mouse permission.",
    visibleWhen: mouseOn,
  },
  {
    key: 'gamepad',
    label: 'Controller type',
    description:
      "The virtual controller the host uses. Automatic matches each device's controller; choosing a type here uses it for all connected controllers.",
    details:
      'Automatic picks DualSense, with adaptive triggers, for PlayStation, Switch Pro for Nintendo, and Xbox Series otherwise, or DualSense when the motion or touchpad options below select it. Waveform-capable DualSense clients use native USB audio haptics when usbip-win2 is installed on the host. The USB/IP option requires that driver and exposes a DualSense audio device. Steam stable can list a virtual Xbox controller twice; the Steam beta lists it once.',
    category: 'input',
    group: 'Controllers',
    control: {
      kind: 'select',
      options: [
        { value: 'auto', label: 'Automatic' },
        { value: 'vhf_xbox', label: 'Xbox Series' },
        { value: 'vhf_xbox_one', label: 'Xbox One' },
        { value: 'vhf_ds4', label: 'DualShock 4' },
        { value: 'vhf_ds5', label: 'DualSense' },
        { value: 'usbip_ds5', label: 'DualSense with waveform haptics (USB/IP)' },
        { value: 'vhf_switch', label: 'Switch Pro' },
      ],
    },
    // input_policy.rs gamepad_profile: Sunshine, Apollo and Vibepollo names.
    aliases: { spellings: { x360: 'vhf_xbox_one', ds4: 'vhf_ds4', ds5: 'vhf_ds5', vhf: 'auto' } },
    default: 'auto',
    inactiveHint: "Turn on Controllers under Input to use this setting. Paired devices also need the Controllers permission.",
    visibleWhen: controllerOn,
  },
  {
    key: 'motion_as_ds4',
    label: 'PlayStation controller for motion controls',
    description:
      'With Automatic, motion sensors select DualSense. Turn it off to ignore motion sensors when choosing.',
    details: 'This includes Xbox-type devices such as Steam Deck. Nintendo devices stay Switch Pro.',
    category: 'input',
    group: 'Controllers',
    control: { kind: 'toggle' },
    default: true,
    inactiveHint: "Turn on Controllers under Input to use this setting. Paired devices also need the Controllers permission.",
    visibleWhen: controllerOn,
  },
  {
    key: 'touchpad_as_ds4',
    label: 'PlayStation controller for touchpads',
    description:
      'With Automatic, a touchpad selects DualSense. Turn it off to ignore the touchpad when choosing.',
    details: 'Nintendo devices stay Switch Pro.',
    category: 'input',
    group: 'Controllers',
    control: { kind: 'toggle' },
    default: true,
    inactiveHint: "Turn on Controllers under Input to use this setting. Paired devices also need the Controllers permission.",
    visibleWhen: controllerOn,
  },
  {
    key: 'forward_rumble',
    label: 'Vibration',
    description: "Send vibration from games back to the device's controller.",
    category: 'input',
    group: 'Controllers',
    control: { kind: 'toggle' },
    default: true,
    inactiveHint: "Turn on Controllers under Input to use this setting. Paired devices also need the Controllers permission.",
    visibleWhen: controllerOn,
  },
  {
    key: 'back_button_timeout',
    label: 'Hold Back for Guide',
    description:
      'Holding Back (Select) this long presses the Guide (Home) button instead, for controllers without one. -1 turns this off.',
    category: 'input',
    group: 'Controllers',
    control: { kind: 'number', min: -1, max: 60000, step: 1, unit: 'ms' },
    default: -1,
    inactiveHint: "Turn on Controllers under Input to use this setting. Paired devices also need the Controllers permission.",
    visibleWhen: controllerOn,
  },
  {
    key: 'steam_deck_controller',
    label: 'Steam Deck controller',
    description:
      "How a Steam Deck's controls reach the host. Steam Deck lets Steam on the host see a real Steam Deck and apply its Steam Input layout.",
    details:
      'Steam Deck attaches a real Steam Deck controller through usbip-win2, with its trackpads, motion sensors and back grips; it needs usbip-win2 installed. Automatic does that while Steam is running on the host. Virtual controller always uses the virtual DualSense or Xbox controller.',
    category: 'input',
    group: 'Steam Deck and back grips',
    control: {
      kind: 'select',
      options: [
        { value: 'auto', label: 'Automatic' },
        { value: 'steam_deck', label: 'Steam Deck' },
        { value: 'virtual_pad', label: 'Virtual controller' },
      ],
    },
    default: 'auto',
    inactiveHint: "Turn on Controllers under Input to use this setting. Paired devices also need the Controllers permission.",
    visibleWhen: controllerOn,
  },
  backGrip('back_grip_l4', 'L4', 'upper left'),
  backGrip('back_grip_r4', 'R4', 'upper right'),
  backGrip('back_grip_l5', 'L5', 'lower left'),
  backGrip('back_grip_r5', 'R5', 'lower right'),
];

const commands: Setting[] = [
  {
    key: 'global_prep_cmd',
    label: 'Preparation commands',
    description:
      'Commands run before every app starts (do) and after it closes (undo). If a do command fails, the app does not start; apps can opt out in their settings.',
    category: 'commands',
    group: 'Around every app',
    control: { kind: 'commands' },
    default: [],
  },
  {
    key: 'global_state_cmd',
    label: 'Pause and resume commands',
    description:
      "Commands run when any app's stream resumes (do) and when it pauses (undo), before the app's own state commands. Apps can opt out in their settings.",
    category: 'commands',
    group: 'Around every app',
    control: { kind: 'commands' },
    default: [],
  },
  {
    key: 'server_cmd',
    label: 'Host commands',
    description:
      'Commands a device can run on the host from its stream menu, in streaming apps that support it, such as Artemis. Only devices with the Host commands permission see them.',
    category: 'commands',
    group: 'From devices',
    control: { kind: 'server-commands' },
    default: [],
  },
];

export const settings: Setting[] = [...general, ...network, ...audio, ...input, ...commands];
