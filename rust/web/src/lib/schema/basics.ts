// Settings for the general, network, audio, input and commands categories.
// Defaults are the ones the Rust host falls back to when a key is unset.
import type { ConfigValue } from '../api';
import type { Option, Setting } from '../settings-types';

type Values = Record<string, ConfigValue>;

/** A boolean read as the host reads it; anything unrecognised keeps the default. */
export function on(values: Values, key: string, fallback: boolean): boolean {
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
    label: 'PC name',
    description:
      'The name your devices show for this PC. Empty uses its Windows name; local network discovery picks up a change after a restart.',
    category: 'general',
    group: 'Name and language',
    control: { kind: 'text', placeholder: "This PC's name" },
    default: '',
    defaultText: 'Its Windows name',
  },
  {
    key: 'locale',
    label: 'Fallback console language',
    description: 'Language used by the fallback console and reported to connected tools. This console is currently available in English only.',
    category: 'general',
    group: 'Name and language',
    control: { kind: 'select', options: locales },
    // console/i18n.rs locale: any case, - for _.
    aliases: { match: 'dash' },
    default: 'en',
  },
  {
    key: 'enable_pairing',
    label: 'Allow pairing',
    description:
      'Let new devices pair with this PC. Devices already paired keep working either way; while it is off, no other device can ask to pair.',
    category: 'network',
    group: 'Pairing',
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
      "Remove the disconnect, restart and quit actions from the tray icon's menu, so people at the PC cannot stop Rubylight from there.",
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
    label: 'Check for updates',
    description:
      'How often Rubylight looks for a new release; Daily suits almost everyone. Off stops the checks and automatic installation, so you hear of updates only through Check now.',
    details:
      'A custom interval is in seconds, at least 3600 (one hour). Check now under Updates works at any time, even with checks off.',
    category: 'general',
    group: 'Updates',
    control: {
      kind: 'number',
      min: 0,
      step: 1,
      unit: 's',
      presets: [
        { value: 21600, label: 'Every 6 hours' },
        { value: 86400, label: 'Daily' },
        { value: 604800, label: 'Weekly' },
        { value: 0, label: 'Off' },
      ],
    },
    default: 86400,
  },
  {
    key: 'update_check_time',
    label: 'Update check time',
    description:
      "Look for a new release once a day at this time on this PC's clock, in place of the interval. Leave it empty unless checks, and automatic installs, should happen at a set time.",
    details:
      "With Install updates automatically on, a new release installs after that check, once nothing is streaming. If this PC is off, asleep or streaming at that time, the check runs as soon as it can afterwards. The time follows this PC's time zone and daylight saving time. Check for updates set to Off still turns off automatic checks; otherwise the interval is not used while a time is set.",
    category: 'general',
    group: 'Updates',
    control: { kind: 'text', time: true },
    default: '',
  },
  {
    key: 'auto_update',
    label: 'Install updates automatically',
    description:
      'Download and install new releases on their own, only while nothing is streaming or running. Off, the console tells you about an update and you install it when you choose.',
    details: 'Requires the Windows service, and Check for updates on. Only verified releases are downloaded. They install after one minute without streams, pending connections or running apps. Off by default.',
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
    key: 'sleep_after_idle_minutes',
    label: 'Sleep after idle',
    description:
      'Put this PC to sleep when nobody has streamed or used it for this long. A device then has to wake it first, with Wake-on-LAN (see Wake this PC from a device).',
    details:
      'A custom time is 5 to 1440 minutes. Streams, launches, updates, this console, the keyboard and mouse, and apps that keep Windows awake all count as use.',
    category: 'general',
    group: 'Sleep and wake',
    control: {
      kind: 'number',
      min: 0,
      max: 1440,
      step: 1,
      unit: 'min',
      presets: [
        { value: 0, label: 'Never' },
        { value: 15, label: 'After 15 minutes' },
        { value: 30, label: 'After 30 minutes' },
        { value: 60, label: 'After 1 hour' },
        { value: 120, label: 'After 2 hours' },
        { value: 240, label: 'After 4 hours' },
      ],
    },
    default: 0,
  },
  {
    key: 'sleep_while_suspended',
    label: 'Sleep with a suspended stream',
    description:
      'Also let the PC sleep while a stream is suspended or an app launched from a device still runs. Off, either one keeps the PC awake.',
    category: 'general',
    group: 'Sleep and wake',
    control: { kind: 'toggle' },
    default: false,
    inactiveHint: 'Choose a time for Sleep after idle to use this setting.',
    visibleWhen: (values) => Number(values.sleep_after_idle_minutes ?? 0) > 0,
  },
  {
    key: 'lock_on_last_disconnect',
    label: 'Lock Windows when the last device leaves',
    description:
      'Lock this PC 10 seconds after the last device leaves, so it is not left signed in. Unlock it with your Windows password or PIN, from the next stream or at the PC.',
    details:
      'Never during a stream: a device that moves the stream with Continue here or reconnects within 10 seconds keeps the PC unlocked. Unlock it from the next stream, where the lock screen streams like the desktop, or at the PC.',
    category: 'general',
    group: 'Privacy',
    control: { kind: 'toggle' },
    default: false,
  },
  {
    key: 'min_log_level',
    label: 'Log level',
    description:
      'How much detail Rubylight writes to its log; changes apply right away. Leave it on Information; Debug suits a problem report.',
    details:
      'Information includes encoder settings, display setup and timing lines every 5 seconds. Debug adds GPU scheduling, discovery and probe details; Verbose adds everything.',
    category: 'advanced',
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
    description:
      "Lets a compatible device show this PC's CPU and GPU readings (load, temperatures, power) in its overlay during a stream. Leave it on: readings are taken only while a device asks.",
    details: 'Unavailable readings are omitted. Only a device with a running video stream of its own can ask for readings.',
    category: 'general',
    group: 'Hardware metrics',
    control: { kind: 'toggle' },
    default: true,
  },
  {
    key: 'hardware_metrics_interval',
    label: 'Hardware metrics interval',
    description: 'The shortest time between two fresh readings a streaming device can get. Every second suits almost everyone.',
    details: 'A custom time is 500 to 10000 milliseconds.',
    category: 'general',
    group: 'Hardware metrics',
    control: {
      kind: 'number',
      min: 500,
      max: 10000,
      step: 1,
      unit: 'ms',
      presets: [
        { value: 500, label: 'Twice a second' },
        { value: 1000, label: 'Every second' },
        { value: 2000, label: 'Every 2 seconds' },
        { value: 5000, label: 'Every 5 seconds' },
      ],
    },
    default: 1000,
    inactiveHint: "Turn on Send hardware metrics to client to use this setting.",
    visibleWhen: (values) => on(values, 'hardware_metrics', true),
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
    visibleWhen: (values) => on(values, 'hardware_metrics', true),
  },
  {
    key: 'log_path',
    label: 'Log file',
    description: "Where Rubylight writes its log. A relative path is inside Rubylight's configuration folder.",
    category: 'advanced',
    group: 'Logging',
    control: { kind: 'text', placeholder: 'Standard log file', mono: true },
    // Unset is the standard file in the logs folder: the host names it, the console doesn't.
    default: '',
    defaultText: 'The standard log file',
    restart: true,
  },
  {
    key: 'legacy_ordering',
    label: 'App order for older devices',
    description:
      'Turn on only if an older Moonlight lists your apps in the wrong order: it adds invisible characters to app names, which can break scripts and tools that match names exactly.',
    category: 'advanced',
    group: 'Compatibility',
    control: { kind: 'toggle' },
    default: false,
  },
  {
    key: 'envvar_compatibility_mode',
    label: 'Whole-number frame rate for commands',
    description:
      'Give commands SUNSHINE_CLIENT_FPS as a whole number, such as 60 instead of 59.940. Turn it on if a script cannot read fractional frame rates.',
    category: 'advanced',
    group: 'Compatibility',
    control: { kind: 'toggle' },
    default: false,
  },
  {
    key: 'usb_forwarding',
    label: 'USB forwarding',
    description:
      'Let the devices you allow plug their USB devices into this PC during a stream. Each device also needs the USB devices permission, and this PC the usbip-win2 driver.',
    details: 'Needs the usbip-win2 driver on this PC. Windows sees a forwarded device as a real USB device. A device is detached when its stream ends, and every device when this is turned off.',
    category: 'input',
    group: 'USB devices',
    control: { kind: 'toggle' },
    default: false,
  },
  {
    key: 'usb_max_devices',
    label: 'USB devices at once',
    description: 'How many USB devices this PC takes from devices at the same time.',
    category: 'input',
    group: 'USB devices',
    control: { kind: 'number', min: 1, max: 16, step: 1 },
    default: 4,
    inactiveHint: 'Turn on USB forwarding to use this setting.',
    visibleWhen: (values) => on(values, 'usb_forwarding', false),
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
      "Moonlight connects on this port, and Rubylight's other ports are counted from it. Change it only if another program needs these ports; this console's address moves with it.",
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
    description:
      'Also accept devices that connect over IPv6. Leave it on IPv4 only unless devices must reach this PC over IPv6, such as over the internet; ignored when a listen address is set.',
    details: "With IPv4 only, Play over the internet does not offer devices this PC's IPv6 address.",
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
      "Devices can reach this PC only at this one IP address, which must be one of this PC's own; local network discovery uses it too. Leave it empty to listen on every address.",
    details:
      'It must be an IP address, not a name. An address this PC does not have yet, as at boot, is kept: Rubylight retries until the network has it.',
    category: 'network',
    group: 'Ports and addresses',
    control: { kind: 'text', placeholder: 'All addresses', mono: true },
    default: '',
    restart: true,
  },
  {
    key: 'enable_discovery',
    label: 'Local network discovery',
    description:
      'Announce this PC on the local network (mDNS) so Moonlight finds it without typing an address. Leave it on; off, you add this PC on each device by its address.',
    category: 'network',
    group: 'Discovery and internet',
    control: { kind: 'toggle' },
    default: true,
    restart: true,
  },
  {
    key: 'upnp',
    label: 'Automatic port forwarding',
    description:
      'Asks the router to forward the streaming ports (UPnP, PCP or NAT-PMP), opening them to the internet so devices can connect from anywhere. Turn it on only to play away from home.',
    details:
      'Works with routers that have UPnP, PCP or NAT-PMP turned on. The forwards are renewed while Rubylight runs and removed when it stops. With Console access on Any network, the console port is forwarded too.',
    category: 'network',
    group: 'Discovery and internet',
    control: { kind: 'toggle' },
    default: false,
    cautionWhen: (values) => (on(values, 'upnp', false) ? 'The streaming ports are open to the internet while Rubylight runs.' : undefined),
    restart: true,
  },
  {
    key: 'internet_play',
    label: 'Play over the internet',
    description:
      'Lets paired Nightjar devices reach this PC from other networks without port forwarding. The PC contacts public servers that see its public address; the Privacy page lists them.',
    details:
      "Only Nightjar uses the direct path, and only paired devices can read what the PC publishes. A device learns what it needs while it is on your network once. Two networks that both change ports for every destination (symmetric NAT) cannot open a direct path: forward the ports there instead. The PC finds its public address with STUN and meets devices through a public peer-to-peer directory (the Mainline DHT), where it leaves only encrypted notes that its paired devices can read. Nothing is downloaded or shared through it. A device tries your network first, then the PC's IPv6 address, then a port the router maps (Automatic port forwarding), and last a direct path both sides open through their routers. The PC contacts public STUN servers and directory nodes, which see its public address; the Privacy page lists them.",
    category: 'network',
    group: 'Discovery and internet',
    control: { kind: 'toggle' },
    default: false,
    restart: true,
  },
  {
    key: 'internet_play_stun_servers',
    label: 'STUN servers',
    description:
      'The public STUN servers that tell this PC its internet address, as host:port separated by commas. Leave it empty to use stun.moonlight-stream.org:3478 and stun.cloudflare.com:3478.',
    category: 'network',
    group: 'Discovery and internet',
    control: { kind: 'text', placeholder: 'stun.moonlight-stream.org:3478, stun.cloudflare.com:3478', mono: true },
    default: '',
    defaultText: "Moonlight's and Cloudflare's STUN servers",
    restart: true,
    inactiveHint: 'Turn on Play over the internet to use this setting.',
    visibleWhen: (values) => on(values, 'internet_play', false),
  },
  {
    key: 'webrtc_server',
    label: 'WebRTC server',
    description:
      'Let TVs and browsers that speak the Moonlight WebRTC protocol find this PC, pair and stream. Leave it off unless you use one: its signalling (connection setup) is not encrypted.',
    details:
      'Only devices on the local network are accepted. Signalling is plain ws:// on the local network: signing in keeps other devices from using this PC, but does not hide the traffic. Pairing requests appear on the Devices page.',
    category: 'network',
    group: 'WebRTC',
    control: { kind: 'toggle' },
    default: false,
    restart: true,
  },
  {
    key: 'webrtc_port',
    label: 'WebRTC signalling port',
    description:
      'The TCP port TVs connect their WebSocket to. Change it only if another program uses it; discovery always listens on UDP 8000, where TVs send their broadcast.',
    details: 'If something else uses a port, the console shows a warning and the server does not start. Ports are never changed for you.',
    category: 'network',
    group: 'WebRTC',
    control: { kind: 'number', min: 1024, max: 65535, step: 1 },
    default: 8000,
    restart: true,
    inactiveHint: 'Turn on WebRTC server to use this setting.',
    visibleWhen: (values) => on(values, 'webrtc_server', false),
  },
  {
    key: 'webrtc_media_port',
    label: 'WebRTC media port',
    description: 'The UDP port for WebRTC video, audio and input. Change it only if another program uses it.',
    details: "Guests of the console's stream use the next four ports.",
    category: 'network',
    group: 'WebRTC',
    control: { kind: 'number', min: 1024, max: 65535, step: 1 },
    default: 8001,
    restart: true,
    inactiveHint: 'Turn on WebRTC server to use this setting.',
    visibleWhen: (values) => on(values, 'webrtc_server', false),
  },
  {
    key: 'file_transfer',
    label: 'File transfer',
    description:
      'Let paired devices send files to this PC and download them again. Each device also needs the Upload files or Download files permission.',
    details: 'Files are written as the Windows user who is signed in. An upload is saved under a .part name and renamed only once all of it arrived.',
    category: 'network',
    group: 'File transfer',
    control: { kind: 'toggle' },
    default: false,
  },
  {
    key: 'file_transfer_dir',
    label: 'File transfer folder',
    description: "Where uploads go and downloads come from. Leave it empty for Downloads\\Rubylight of the signed-in user.",
    category: 'network',
    group: 'File transfer',
    control: { kind: 'text', placeholder: 'Downloads\\Rubylight', mono: true, browse: 'directory' },
    default: '',
    inactiveHint: 'Turn on File transfer to use this setting.',
    visibleWhen: (values) => on(values, 'file_transfer', false),
  },
  {
    key: 'file_transfer_max_mb',
    label: 'Largest file',
    description: 'Uploads larger than this are refused before they start.',
    category: 'network',
    group: 'File transfer',
    control: { kind: 'number', min: 1, step: 1, unit: 'MiB' },
    default: 4096,
    inactiveHint: 'Turn on File transfer to use this setting.',
    visibleWhen: (values) => on(values, 'file_transfer', false),
  },
  {
    key: 'file_transfer_stream_mbps',
    label: 'Transfer speed while streaming',
    description:
      'Uploads and downloads together slow to this rate while a stream runs, leaving the rest of the network to the stream. 0 lets them run at full speed.',
    category: 'network',
    group: 'File transfer',
    control: { kind: 'number', min: 0, step: 1, unit: 'Mbit/s' },
    default: 50,
    inactiveHint: 'Turn on File transfer to use this setting.',
    visibleWhen: (values) => on(values, 'file_transfer', false),
  },
  {
    key: 'shared_folders',
    label: 'Shared folders',
    description: 'Folders of this PC that devices may browse. Each device sees only the folders you share with it on Devices.',
    details: 'Files are read and written as the Windows user who is signed in, only inside each folder. Transfers keep to the file transfer size limit and speed while streaming. Renaming a folder keeps what devices may do with it; removing it takes it away from every device.',
    category: 'network',
    group: 'Shared folders',
    control: { kind: 'folders' },
    default: '[]',
  },
  {
    key: 'clipboard_image',
    label: 'Copy images',
    description: 'Let devices with clipboard permission copy images to and from this PC, besides text.',
    details: 'Images travel as PNG. Windows programs get a bitmap and a PNG to paste.',
    category: 'network',
    group: 'Clipboard',
    control: { kind: 'toggle' },
    default: true,
  },
  {
    key: 'clipboard_image_max_mb',
    label: 'Largest image',
    description: 'Images larger than this, as PNG, are refused in both directions.',
    category: 'network',
    group: 'Clipboard',
    control: { kind: 'number', min: 1, max: 64, step: 1, unit: 'MiB' },
    default: 16,
    inactiveHint: 'Turn on Copy images to use this setting.',
    visibleWhen: (values) => on(values, 'clipboard_image', true),
  },
  {
    key: 'lan_encryption_mode',
    label: 'Local network encryption',
    description:
      'Encrypts streams on the local network, Tailscale included. Off suits a network you trust: encrypting costs slower devices performance; Required refuses devices that cannot encrypt.',
    details:
      'Required turns away devices that cannot encrypt, and demands encrypted video and audio from the others. Off does not offer video encryption on the local network.',
    category: 'network',
    group: 'Streams',
    control: { kind: 'select', options: encryption },
    default: 0,
  },
  {
    key: 'wan_encryption_mode',
    label: 'Internet encryption',
    description:
      'Encrypts streams outside the local network. Leave it on When the device supports it: Off sends video over the internet unencrypted; Required refuses devices that cannot encrypt.',
    details:
      'Required turns away devices that cannot encrypt, and demands encrypted video and audio from the others. With When the device supports it, a device that cannot encrypt still streams, and the console warns that its stream crosses the internet unencrypted. Off does not offer video encryption. A device that connects through Play over the internet counts as outside the local network; Tailscale counts as the local network.',
    category: 'network',
    group: 'Streams',
    control: { kind: 'select', options: encryption },
    default: 1,
    cautionWhen: (values) =>
      text(values, 'wan_encryption_mode', '1') === '0' ? 'Video goes over the internet unencrypted.' : undefined,
  },
  {
    key: 'ping_timeout',
    label: 'Device timeout',
    description:
      'How long this PC waits without hearing from a device before it ends the stream. Longer rides out dropouts on unreliable networks, but a vanished device holds its stream that long.',
    details: 'A custom time is 1000 to 300000 milliseconds.',
    category: 'network',
    group: 'Streams',
    control: {
      kind: 'number',
      min: 1000,
      max: 300000,
      step: 1,
      unit: 'ms',
      presets: [
        { value: 5000, label: '5 seconds' },
        { value: 10000, label: '10 seconds' },
        { value: 30000, label: '30 seconds' },
        { value: 60000, label: '1 minute' },
        { value: 300000, label: '5 minutes' },
      ],
    },
    default: 10000,
  },
  {
    key: 'client_roaming',
    label: 'Client roaming',
    description:
      'Let a device whose network address changes, such as a phone moving from Wi-Fi to mobile data, keep its stream instead of reconnecting. Leave it on; other devices stream as before.',
    details:
      'Only devices that support it, such as Nightjar, use it. The device proves the new address with the stream key before this PC sends anything there, and this PC then drops what still arrives from the old one. Moonlight clients that do not support it stream as before. Turning it off stops roaming at once; turning it on applies from the next stream.',
    category: 'network',
    group: 'Streams',
    control: { kind: 'toggle' },
    default: true,
  },
  {
    key: 'dscp_tagging',
    label: 'Prioritize stream traffic (DSCP)',
    description:
      'Marks video and audio for local devices (DSCP) so Wi-Fi and switches that honour the marks send them first. Leave it on unless sound crackles or the picture stutters on one router.',
    details:
      'Some consumer routers drop or delay marked packets: turn it off if sound crackles or the picture stutters only on one router. Devices outside the local network are never marked. Video is marked as audio-video traffic (DSCP 40) and audio as voice (DSCP 56), through the Windows qWAVE service, when the device asks for it. Applies from the next stream.',
    category: 'network',
    group: 'Streams',
    control: { kind: 'toggle' },
    default: true,
  },
  {
    key: 'bandwidth_probe',
    label: 'Bandwidth test',
    description:
      'Lets paired devices time a short download to measure the network before they pick a bitrate. Leave it on: the device decides whether to run it, and off refuses the download.',
    category: 'network',
    group: 'Streams',
    control: { kind: 'toggle' },
    default: true,
  },
  {
    key: 'origin_web_ui_allowed',
    label: 'Console access',
    description:
      'Which addresses may open this console and use its API. Leave it on Local network: Any network also accepts addresses on the internet, and This PC only shuts out your other devices.',
    details:
      'Local network includes private addresses and Tailscale (100.64.0.0/10). With Any network and UPnP on, the console port is forwarded too.',
    category: 'advanced',
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
    cautionWhen: (values) =>
      text(values, 'origin_web_ui_allowed', 'lan') === 'wan'
        ? 'The console and its API accept addresses on the internet.'
        : undefined,
  },
  {
    key: 'csrf_allowed_origins',
    label: 'Trusted origins',
    description:
      'Other web addresses allowed to make changes through this console. Add one only when you reach the console through a reverse proxy.',
    details: 'Write each one as scheme and host, such as https://stream.example.com.',
    category: 'advanced',
    group: 'Console',
    control: { kind: 'list', placeholder: 'https://stream.example.com' },
    default: [],
  },
  {
    key: 'session_token_ttl_seconds',
    label: 'Sign-in token lifetime',
    description:
      'How long a console sign-in token is valid before the browser renews it on its own. Leave it at 2 hours unless you want tokens replaced more or less often.',
    details:
      'A custom time is 60 to 604800 seconds. Without "Keep me signed in", a sign-in ends after this time or one day, whichever is longer.',
    category: 'advanced',
    group: 'Console',
    control: {
      kind: 'number',
      min: 60,
      max: 604800,
      step: 1,
      unit: 's',
      presets: [
        { value: 3600, label: '1 hour' },
        { value: 7200, label: '2 hours' },
        { value: 28800, label: '8 hours' },
        { value: 86400, label: '1 day' },
        { value: 604800, label: '1 week' },
      ],
    },
    default: 7200,
  },
  {
    key: 'remember_me_refresh_token_ttl_seconds',
    label: 'Remembered sign-in lifetime',
    description:
      'How long a browser signed in with "Keep me signed in" stays signed in, up to 366 days. Lower it on shared computers.',
    details: 'A custom time is 60 to 31622400 seconds.',
    category: 'advanced',
    group: 'Console',
    control: {
      kind: 'number',
      min: 60,
      max: 31622400,
      step: 1,
      unit: 's',
      presets: [
        { value: 86400, label: '1 day' },
        { value: 604800, label: '1 week' },
        { value: 2592000, label: '30 days' },
        { value: 7776000, label: '90 days' },
        { value: 31622400, label: '366 days' },
      ],
    },
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
      "Send this PC's sound to the device. Turn it off when a stream is only used as an extra monitor and should stay silent.",
    category: 'audio',
    group: 'Sound',
    control: { kind: 'toggle' },
    default: true,
  },
  {
    key: 'audio_capture_mode',
    label: 'Audio to stream',
    description:
      'System audio suits almost everyone. Game only streams the launched game alone, without browsers and notifications, but in stereo only and silent when the game cannot be verified.',
    details:
      'Reconnect to apply. Game only captures the launched game and its children. It requires a verified game process; when capture is unavailable, the stream sends silence with a session warning. Game only uses stereo and leaves the Windows default playback device unchanged: unless the device keeps sound on this PC, the game alone plays on the virtual speakers, and without virtual speakers it stays audible on this PC and the stream warns.',
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
    key: 'audio_passthrough',
    label: 'PCM and Dolby Digital audio',
    description:
      'Offers uncompressed PCM, Dolby Digital and Dolby Digital Plus to devices that ask for it, for TVs and receivers that decode it themselves; leave it on. Other devices keep Opus.',
    details:
      'PCM carries stereo up to 7.1.4 and takes about 1.5 Mbit/s per stereo pair. Dolby Digital and Dolby Digital Plus carry stereo and 5.1 in 32 ms frames, so their sound arrives a little later than Opus. A device that asks for a format this PC cannot encode is refused with the reason.',
    category: 'audio',
    group: 'Sound',
    control: { kind: 'toggle' },
    default: true,
    inactiveHint: "Turn on Stream audio to use this setting.",
    visibleWhen: streamsAudio,
  },
  {
    key: 'audio_sink',
    label: 'Audio device',
    description:
      'The audio output captured when the streaming device keeps sound playing on this PC, or when no virtual speakers exist. Leave it empty to use the Windows default output.',
    details:
      'If the chosen output is unavailable while a device keeps sound on this PC, or with Capture without switching on, the stream sends silence, says why and keeps retrying.',
    category: 'audio',
    group: 'Audio devices',
    control: { kind: 'audio' },
    default: '',
    inactiveHint: "Turn on Stream audio and choose System audio under Audio to stream to use this setting.",
    visibleWhen: systemAudio,
  },
  {
    key: 'virtual_sink',
    label: 'Virtual speakers',
    description:
      "The audio output made the Windows default during a stream, so sound goes to the device instead of the PC's speakers. Leave it empty to use Steam Streaming Speakers when installed.",
    details:
      'An output chosen here is used even when the streaming device keeps sound on this PC. If it is unavailable, the stream sends silence, says why and keeps retrying.',
    category: 'audio',
    group: 'Audio devices',
    control: { kind: 'audio' },
    default: '',
    inactiveHint: "Turn on Stream audio and choose System audio under Audio to stream to use this setting.",
    visibleWhen: systemAudio,
  },
  {
    key: 'audio_sink_capture_only',
    label: 'Capture without switching',
    description:
      'Capture the audio device without making it the Windows default output. Leave it off unless you want to send apps to that device yourself: only what plays there reaches the stream.',
    details: 'Applies only when an audio device is set and virtual speakers are not.',
    category: 'audio',
    group: 'Audio devices',
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
      'Capture whichever device is the Windows default, even if it changes during a stream. Leave it on; turn it off only to stay on the device chosen when the stream started.',
    details: 'Applies only without virtual speakers. When on, capture is also retried when it fails.',
    category: 'audio',
    group: 'Audio devices',
    control: { kind: 'toggle' },
    default: true,
    inactiveHint: "Turn on Stream audio and choose System audio under Audio to stream to use this setting.",
    visibleWhen: systemAudio,
  },
  {
    key: 'keep_sink_default',
    label: 'Keep virtual speakers as default',
    description:
      'If another device becomes the Windows default mid-stream, switch back to the virtual speakers so the stream keeps its sound. Leave it on; the device you picked returns afterwards.',
    category: 'audio',
    group: 'Virtual speakers',
    control: { kind: 'toggle' },
    default: true,
    inactiveHint: "Turn on Stream audio and choose System audio under Audio to stream to use this setting.",
    visibleWhen: systemAudio,
  },
  {
    key: 'hold_sink_while_paused',
    label: 'Keep virtual speakers while paused',
    description:
      "Keeps this PC silent when a device disconnects and its game keeps running: the virtual speakers stay the Windows default until a device returns. Off, the PC plays the game's sound.",
    details:
      'The device you picked is restored when the game closes or you choose End session.',
    category: 'audio',
    group: 'Virtual speakers',
    control: { kind: 'toggle' },
    default: false,
    inactiveHint: "Turn on Stream audio and choose System audio under Audio to stream to use this setting.",
    visibleWhen: systemAudio,
  },
  {
    key: 'install_steam_audio_drivers',
    label: 'Install Steam audio drivers',
    description:
      "When Steam is installed, install its streaming speakers and microphone when a stream needs them. Leave it on: they keep the PC silent and carry surround and a device's microphone.",
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
      'Let a streaming device talk through this PC with its own microphone, where the client supports it. Leave it on: a device sends its microphone only when you turn that on there.',
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
const rumbleOn = (values: Values) => controllerOn(values) && on(values, 'forward_rumble', true);
const audioHapticsOn = (values: Values) => rumbleOn(values) && on(values, 'audio_haptics', false);

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
    inactiveHint: "Turn on Controllers under Input devices to use this setting. Paired devices also need the Controllers permission.",
    visibleWhen: controllerOn,
  };
}

const input: Setting[] = [
  {
    key: 'keyboard',
    label: 'Keyboard',
    description: 'Let devices type on this PC.',
    category: 'input',
    group: 'Input devices',
    control: { kind: 'toggle' },
    default: true,
  },
  {
    key: 'mouse',
    label: 'Mouse',
    description: 'Let devices use the mouse on this PC. Pen and touch input need this too.',
    category: 'input',
    group: 'Input devices',
    control: { kind: 'toggle' },
    default: true,
  },
  {
    key: 'controller',
    label: 'Controllers',
    description: 'Let devices use game controllers on this PC.',
    category: 'input',
    group: 'Input devices',
    control: { kind: 'toggle' },
    default: true,
  },
  {
    key: 'enable_input_only_mode',
    label: 'Remote input entry',
    description:
      "Add a Remote Input entry to every device's app list. It sends keyboard, mouse and controller input to this PC without video, for example from a second device.",
    category: 'input',
    group: 'Input devices',
    control: { kind: 'toggle' },
    default: false,
  },
  {
    key: 'always_send_scancodes',
    label: 'Send scan codes',
    description:
      'Send keys as scan codes (the codes a real keyboard sends), which some games need to see keyboard input at all. Leave it on; turn it off only if a device types the wrong characters.',
    details:
      "Scan codes follow Keyboard layout: by default the layout the device reports, or this PC's when the device reports none. Turn this off only if a device that reports no layout types the wrong characters.",
    category: 'input',
    group: 'Keyboard',
    control: { kind: 'toggle' },
    default: true,
    inactiveHint: "Turn on Keyboard under Input devices to use this setting. Paired devices also need the Keyboard permission.",
    visibleWhen: keyboardOn,
  },
  {
    key: 'key_rightalt_to_key_win',
    label: 'Right Alt as Windows key',
    description:
      'Treat the right Alt key as the Windows key, for devices that cannot send the Windows key. The right Alt key then no longer works as Alt or AltGr.',
    category: 'input',
    group: 'Keyboard',
    control: { kind: 'toggle' },
    default: false,
    inactiveHint: "Turn on Keyboard under Input devices to use this setting. Paired devices also need the Keyboard permission.",
    visibleWhen: keyboardOn,
  },
  {
    key: 'key_repeat_delay',
    label: 'Key repeat delay',
    description: 'How long a held key waits before it starts repeating. 0 turns key repeat off on this PC.',
    category: 'input',
    group: 'Keyboard',
    control: { kind: 'number', min: 0, max: 60000, step: 1, unit: 'ms' },
    default: 500,
    inactiveHint: "Turn on Keyboard under Input devices to use this setting. Paired devices also need the Keyboard permission.",
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
    inactiveHint: "Turn on Keyboard under Input devices to use this setting. Paired devices also need the Keyboard permission.",
    visibleWhen: keyboardOn,
  },
  {
    key: 'keybindings',
    label: 'Key remapping (JSON)',
    description: 'Makes a key from the device press a different key on this PC. Leave it empty unless a key should act as another one.',
    details:
      'One list of Windows virtual-key codes in pairs, each key from the device followed by the key this PC presses instead, such as ["0x10", "0xA0"]. Shift, Ctrl and Alt already map to their left-hand keys.',
    category: 'input',
    group: 'Key remapping',
    control: { kind: 'json' },
    default: [],
    inactiveHint: "Turn on Keyboard under Input devices to use this setting. Paired devices also need the Keyboard permission.",
    visibleWhen: keyboardOn,
  },
  {
    key: 'keyboard_follows_focus',
    label: 'Keyboard follows text fields',
    description:
      "Show the device's keyboard when a text field on this PC gets the focus, and hide it after, on devices that support it. Turn it off if the keyboard opens when you do not want it.",
    category: 'input',
    group: 'Keyboard',
    control: { kind: 'toggle' },
    default: true,
    inactiveHint: "Turn on Keyboard under Input devices to use this setting. Paired devices also need the Keyboard permission.",
    visibleWhen: keyboardOn,
  },
  {
    key: 'text_input_surrounding_text',
    label: 'Send the text around the cursor',
    description:
      "Send the text around the cursor to the device's keyboard for its suggestions, never from password fields. Leave it off if text on this PC should not reach the device's keyboard.",
    category: 'input',
    group: 'Keyboard',
    control: { kind: 'toggle' },
    default: false,
    inactiveHint: 'Turn on Keyboard under Input devices and Keyboard follows text fields to use this setting.',
    visibleWhen: (values) => keyboardOn(values) && on(values, 'keyboard_follows_focus', true),
  },
  {
    key: 'keyboard_layout',
    label: 'Keyboard layout',
    description:
      "Which layout turns the device's keys into scan codes. Leave it on client, the layout the device reports, unless keys type wrong characters; us is US English, or enter a layout id.",
    details: 'A layout id is a Windows keyboard layout id of eight hex digits, such as 00000407.',
    category: 'input',
    group: 'Keyboard',
    control: { kind: 'text', placeholder: 'client, us or 00000407', mono: true },
    default: 'client',
    inactiveHint: "Turn on Keyboard under Input devices to use this setting. Paired devices also need the Keyboard permission.",
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
    inactiveHint: "Turn on Mouse under Input devices to use this setting. Paired devices also need the Mouse permission.",
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
    inactiveHint: "Turn on Mouse under Input devices to use this setting. Paired devices also need the Mouse permission.",
    visibleWhen: mouseOn,
  },
  {
    key: 'mouse_mode',
    label: 'Mouse input method',
    description:
      'How this PC moves its mouse; Automatic suits almost everyone. Virtual mouse is a USB mouse games read like a real one; Injection is Windows input some games and anti-cheat ignore.',
    details:
      'Automatic uses the virtual mouse when the USB audio and haptics driver is installed, and injection otherwise. The virtual mouse needs usbip-win2, the driver the setup installs with USB audio and haptics. Without it, Virtual mouse uses injection and the stream says why. Measured on an RX 7900 XT PC, a move took about 0.25 ms from input handling on this PC to the game through the virtual mouse and 0.19 ms through injection (median). A game or device can choose its own method under Games or Devices.',
    category: 'input',
    group: 'Mouse, pen and touch',
    control: {
      kind: 'select',
      options: [
        { value: 'auto', label: 'Automatic' },
        { value: 'virtual_mouse', label: 'Virtual mouse' },
        { value: 'injection', label: 'Injection' },
      ],
    },
    // input_policy.rs MouseMode::parse: Foundation Sunshine's names.
    aliases: { spellings: { vmouse: 'virtual_mouse', sendinput: 'injection' } },
    default: 'auto',
    inactiveHint: "Turn on Mouse under Input devices to use this setting. Paired devices also need the Mouse permission.",
    visibleWhen: mouseOn,
  },
  {
    key: 'native_touchpad',
    label: 'Touchpad gestures from devices',
    description:
      "Replays a device's physical touchpad on a virtual Windows precision touchpad, so Windows' scrolling and gestures work. Leave it on; off, the touchpad moves the mouse instead.",
    details:
      "Off, or without the USB audio and haptics driver, the device's touchpad moves the mouse instead.",
    category: 'input',
    group: 'Mouse, pen and touch',
    control: { kind: 'toggle' },
    default: true,
    inactiveHint: "Turn on Mouse under Input devices to use this setting. Paired devices also need the Mouse permission.",
    visibleWhen: mouseOn,
  },
  {
    key: 'gamepad',
    label: 'Controller type',
    description:
      "The virtual controller games see. Leave it on Automatic, which matches each device's controller, unless a game needs a certain type; USB/IP and ViGEmBus types need a driver.",
    details:
      'A type chosen here is used for all connected controllers; a device can set its own type in Devices, which wins over this one. A game that understands only Xbox controllers cannot see a DualSense. The DualSense USB/IP option and Switch 2 Pro (USB/IP) require usbip-win2. Xbox 360 (ViGEmBus) needs ViGEmBus installed separately, for tools such as reWASD that may not see the other types; without it controllers stop working and the stream says why. Automatic picks DualSense, with adaptive triggers, for PlayStation, Switch Pro for Nintendo, and Xbox Series otherwise, or DualSense when the motion or touchpad options below select it. Waveform-capable DualSense clients use native USB audio haptics when usbip-win2 is installed on the host. The DualSense USB/IP option exposes a DualSense audio device. A client\'s Switch 2 Pro Controller arrives as a Switch 2 Pro Controller with its C button, grip buttons, motion, rumble and battery when usbip-win2 is installed, and as Switch Pro otherwise; Switch 2 Pro (USB/IP) gives every controller one. Steam Controller presents a 2026 Steam Controller with both trackpads, grip and stick touch, motion and a battery; a client that has the real controller also gets Steam\'s pad haptics and settings, and others get rumble. Steam stable can list a virtual Xbox controller twice; the Steam beta lists it once.',
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
        { value: 'usbip_switch2', label: 'Switch 2 Pro (USB/IP)' },
        { value: 'vhf_switch', label: 'Switch Pro' },
        { value: 'vhf_steam', label: 'Steam Controller' },
        { value: 'vigem_x360', label: 'Xbox 360 (ViGEmBus)' },
      ],
    },
    // input_policy.rs gamepad_profile: Sunshine, Apollo and Vibepollo names.
    aliases: { spellings: { x360: 'vhf_xbox_one', ds4: 'vhf_ds4', ds5: 'vhf_ds5', vhf: 'auto' } },
    default: 'auto',
    inactiveHint: "Turn on Controllers under Input devices to use this setting. Paired devices also need the Controllers permission.",
    visibleWhen: controllerOn,
  },
  {
    key: 'motion_as_ds4',
    label: 'PlayStation controller for motion controls',
    description:
      'With Controller type on Automatic, a gyro makes the controller a DualSense, so games get motion controls. Turn it off for games that see only Xbox controllers, which have no gyro.',
    details:
      'This includes Xbox-type devices such as Steam Deck. Nintendo devices stay Switch Pro. With this off, a touchpad still selects DualSense while PlayStation controller for touchpads is on.',
    category: 'input',
    group: 'Controllers',
    control: { kind: 'toggle' },
    default: true,
    inactiveHint: "Turn on Controllers under Input devices to use this setting. Paired devices also need the Controllers permission.",
    visibleWhen: controllerOn,
  },
  {
    key: 'touchpad_as_ds4',
    label: 'PlayStation controller for touchpads',
    description:
      'With Controller type on Automatic, a touchpad makes the controller a DualSense, so games get the touchpad. Turn it off for games that see only Xbox controllers, which have none.',
    details:
      'Nintendo devices stay Switch Pro. With this off, motion sensors still select DualSense while PlayStation controller for motion controls is on.',
    category: 'input',
    group: 'Controllers',
    control: { kind: 'toggle' },
    default: true,
    inactiveHint: "Turn on Controllers under Input devices to use this setting. Paired devices also need the Controllers permission.",
    visibleWhen: controllerOn,
  },
  {
    key: 'enable_dsu_server',
    label: 'Motion server for emulators (DSU)',
    description:
      "Lets emulators such as Cemu, Dolphin and Citra read each streamed controller's gyro, buttons and touch (DSU, cemuhook). Leave it off unless you use one: it has no password.",
    details:
      "The server answers on every network interface without a password, so other devices on your network can read controller input too; the Windows Firewall blocks them unless you allow the port. It serves each streamed controller's gyroscope, accelerometer, buttons and touch. In the emulator, choose DSU or cemuhook motion and the server 127.0.0.1 with the port below. Controllers take slots 1 to 4 in the order they connect.",
    category: 'input',
    group: 'Controllers',
    control: { kind: 'toggle' },
    default: false,
    cautionWhen: (values) =>
      on(values, 'enable_dsu_server', false)
        ? 'No password: other devices on your network can read controller input if the Windows Firewall allows the port.'
        : undefined,
    restart: true,
    inactiveHint: "Turn on Controllers under Input devices to use this setting. Paired devices also need the Controllers permission.",
    visibleWhen: controllerOn,
  },
  {
    key: 'dsu_server_port',
    label: 'Motion server port',
    description: 'The UDP port emulators connect to. 26760 is the port they expect unless told otherwise.',
    details: 'If something else uses the port, the console shows a warning and the server does not start.',
    category: 'input',
    group: 'Controllers',
    control: { kind: 'number', min: 1024, max: 65535, step: 1 },
    default: 26760,
    restart: true,
    inactiveHint: 'Turn on Controllers under Input devices and Motion server for emulators (DSU) to use this setting.',
    visibleWhen: (values) => controllerOn(values) && on(values, 'enable_dsu_server', false),
  },
  {
    key: 'forward_rumble',
    label: 'Vibration',
    description: "Send vibration from games back to the device's controller.",
    category: 'input',
    group: 'Controllers',
    control: { kind: 'toggle' },
    default: true,
    inactiveHint: "Turn on Controllers under Input devices to use this setting. Paired devices also need the Controllers permission.",
    visibleWhen: controllerOn,
  },
  {
    key: 'audio_haptics',
    label: 'Haptics from game audio',
    description:
      "A DualSense on the device feels the game's bass and impacts when the game sends it no vibration of its own. A matter of taste: turn it on to try it; changes apply during a stream.",
    details:
      "Needs a client with DualSense waveform haptics. It stops the moment the game vibrates the controller and returns a minute after the game's last vibration.",
    category: 'input',
    group: 'Controllers',
    control: { kind: 'toggle' },
    default: false,
    inactiveHint: "Turn on Controllers under Input devices and Vibration to use this setting.",
    visibleWhen: rumbleOn,
  },
  {
    key: 'audio_haptics_strength',
    label: 'Audio haptics strength',
    description: 'How strongly the bass drives the actuators. 0 is off; above 100 % quiet scenes are felt more but loud ones clip.',
    category: 'input',
    group: 'Controllers',
    control: { kind: 'number', min: 0, max: 200, step: 5, unit: '%' },
    default: 100,
    inactiveHint: 'Turn on Haptics from game audio to use this setting.',
    visibleWhen: audioHapticsOn,
  },
  {
    key: 'audio_haptics_curve',
    label: 'Audio haptics response',
    description: 'Linear follows the sound; Soft lifts quiet bass, and Punchy keeps only the big hits.',
    category: 'input',
    group: 'Controllers',
    control: {
      kind: 'select',
      options: [
        { value: 'linear', label: 'Linear' },
        { value: 'soft', label: 'Soft' },
        { value: 'punchy', label: 'Punchy' },
      ],
    },
    default: 'linear',
    inactiveHint: 'Turn on Haptics from game audio to use this setting.',
    visibleWhen: audioHapticsOn,
  },
  {
    key: 'audio_haptics_gate',
    label: 'Audio haptics noise gate',
    description: 'Bass quieter than this is not felt, so music and ambience under it stay still. Raise it if the controller hums.',
    category: 'input',
    group: 'Controllers',
    control: { kind: 'number', min: -80, max: -10, step: 1, unit: 'dBFS' },
    default: -50,
    inactiveHint: 'Turn on Haptics from game audio to use this setting.',
    visibleWhen: audioHapticsOn,
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
    inactiveHint: "Turn on Controllers under Input devices to use this setting. Paired devices also need the Controllers permission.",
    visibleWhen: controllerOn,
  },
  {
    key: 'steam_deck_controller',
    label: 'Steam Deck controller',
    description:
      "How a Steam Deck's controls reach this PC. Automatic suits almost everyone: with Steam running here it is a real Steam Deck with its Steam Input layout, else a virtual controller.",
    details:
      'Steam Deck attaches a real Steam Deck controller through usbip-win2, with its trackpads, motion sensors and back grips; it needs usbip-win2 installed, and without it the Deck gets the virtual controller. Steam on this PC then sees a real Steam Deck and applies its Steam Input layout. Automatic does that while Steam is running on the host. Virtual controller always uses the virtual DualSense or Xbox controller.',
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
    inactiveHint: "Turn on Controllers under Input devices to use this setting. Paired devices also need the Controllers permission.",
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
      'Commands run before every app starts (do) and after it closes (undo); if a do command fails, the app does not start. Apps can opt out in their settings.',
    category: 'commands',
    group: 'Commands around every app',
    control: { kind: 'commands' },
    default: [],
  },
  {
    key: 'global_state_cmd',
    label: 'Pause and resume commands',
    description:
      "Commands run when any app's stream resumes (do) and when it pauses (undo), before the app's own state commands. Apps can opt out in their settings.",
    category: 'commands',
    group: 'Commands around every app',
    control: { kind: 'commands' },
    default: [],
  },
  {
    key: 'server_cmd',
    label: 'PC commands',
    description:
      'Commands a device can run on this PC from its stream menu, in streaming apps that support it. Only devices with the PC commands permission see them.',
    category: 'commands',
    group: 'Commands from devices',
    control: { kind: 'server-commands' },
    default: [],
  },
];

export const settings: Setting[] = [...general, ...network, ...audio, ...input, ...commands];
