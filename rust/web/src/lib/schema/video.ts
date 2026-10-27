// Settings for the categories named in settings-schema.ts: video and encoders.
import type { ConfigValue } from '../api';
import type { Aliases, Setting } from '../settings-types';
import { SETTING, defaultPreset, presets } from '../stream-presets';

type Values = Record<string, ConfigValue>;

/** A text setting as the host reads it: unset or blank means the default. */
function read(values: Values, key: string, fallback: string): string {
  const value = values[key];
  if (value === undefined || value === null) return fallback;
  const text = String(value).trim();
  return text === '' ? fallback : text;
}

/** A boolean setting, accepting the spellings the host accepts. */
function flag(values: Values, key: string, fallback: boolean): boolean {
  const value = values[key];
  if (typeof value === 'boolean') return value;
  if (value === undefined || value === null) return fallback;
  switch (String(value).trim().replace(/^"(.*)"$/, '$1').toLowerCase()) {
    case 'true':
    case 'yes':
    case '1':
    case 'enable':
    case 'enabled':
    case 'on':
      return true;
    case 'false':
    case 'no':
    case '0':
    case 'disable':
    case 'disabled':
    case 'off':
      return false;
    default:
      return fallback;
  }
}

const capture = (values: Values) => read(values, 'capture', 'auto').toLowerCase();
const gridPacing = (values: Values) =>
  ['grid', 'fixed'].includes(read(values, 'frame_pacing', 'arrival').toLowerCase());

const video: Setting[] = [
  {
    key: SETTING,
    label: 'Stream preset',
    description:
      'Fills in the AMD encoder settings you have not set yourself. Settings you change below always win.',
    details:
      'Balanced is the host\'s defaults. Lowest latency limits every frame, recovery keyframes included, to one frame\'s share of the bitrate. Reference quality leaves recovery keyframes unlimited, which suits a fast, steady link. No preset changes the rate control. Each value comes from our own measurements in PERFORMANCE.md. A device or an app can choose its own preset.',
    category: 'video',
    group: 'Preset',
    control: {
      kind: 'select',
      options: presets.map((preset) => ({ value: preset.id, label: preset.label })),
    },
    aliases: { match: 'dash' },
    default: defaultPreset,
  },
  {
    key: 'capture',
    label: 'Capture method',
    description:
      'Automatic uses direct driver push on Rubylight virtual displays and Windows Graphics Capture on other displays.',
    details:
      'Direct driver push receives frames from Rubylight\'s own virtual display driver. If unavailable, it falls back to Windows Graphics Capture, then Desktop Duplication if needed. Choosing Windows Graphics Capture or Desktop Duplication bypasses driver push.',
    category: 'video',
    group: 'Capture',
    control: {
      kind: 'select',
      options: [
        { value: 'auto', label: 'Automatic' },
        { value: 'idd-push', label: 'Direct driver push' },
        { value: 'ddx', label: 'Desktop Duplication' },
        { value: 'wgc', label: 'Windows Graphics Capture' },
      ],
    },
    // framegen.rs: any case; Vibepollo's wgcc, and dxgi, which the fallback console writes.
    aliases: { match: 'case', spellings: { wgcc: 'wgc', dxgi: 'ddx' } },
    default: 'auto',
  },
  {
    key: 'capture_poll_interval_us',
    label: 'Capture poll interval',
    description:
      'How long screen capture waits before checking again when no new frame is ready. Shorter waits notice new frames sooner and use more CPU time.',
    category: 'video',
    group: 'Capture troubleshooting',
    control: { kind: 'number', min: 100, max: 1000, step: 50, unit: 'µs' },
    default: 500,
    advanced: true,
  },
  {
    key: 'idd_push_fence_ring',
    label: 'GPU fence transport',
    description:
      'On by default. Turn it off to use the keyed-mutex ring for frames from the Rubylight display driver.',
    details:
      'Needs a compatible Rubylight display driver and compute conversion on AMD. Without them, capture uses the keyed-mutex ring and the console shows a warning. Physical displays and other capture methods are unaffected. End all streams before changing it.',
    category: 'video',
    group: 'Capture troubleshooting',
    control: { kind: 'toggle' },
    default: true,
    visibleWhen: (values) => ['auto', 'idd-push'].includes(capture(values)),
    advanced: true,
  },
  {
    key: 'capture_predictive_poll',
    label: 'Predictive polling',
    description:
      'With Desktop Duplication, checks for frames more often when the next one is due. Can catch frames sooner in games with a steady frame rate.',
    details: 'The expected time of the next frame comes from the recent frame rhythm.',
    category: 'video',
    group: 'Capture troubleshooting',
    control: { kind: 'toggle' },
    default: false,
    inactiveHint: "Applies to Desktop Duplication, including fallback. Direct driver push and Windows Graphics Capture ignore it.",
    visibleWhen: (values) => !['wgc', 'wgcc', 'idd-push'].includes(capture(values)),
    advanced: true,
  },
  {
    key: 'wgc_slot_aligned_publish',
    label: 'Align capture with encoding',
    description:
      'Holds a new frame until just before the next encode if a newer one could arrive first. Helps when the display refreshes much faster than the stream.',
    details: 'Applies to Windows Graphics Capture. Variable refresh rate (VRR) streams ignore it.',
    category: 'video',
    group: 'Capture troubleshooting',
    control: { kind: 'toggle' },
    default: false,
    inactiveHint: "Only affects Windows Graphics Capture, including fallback. Direct driver push, Desktop Duplication and VRR streams ignore it.",
    visibleWhen: (values) => !['ddx', 'dxgi', 'idd-push'].includes(capture(values)),
    advanced: true,
  },
  {
    key: 'wgc_direct_encoder_input',
    label: 'Encode from GPU memory',
    description:
      'Hands captured frames straight to the encoder on the GPU. Turn off only to troubleshoot an encoder.',
    details: 'When off, frames are copied through system memory, which adds latency and CPU load.',
    category: 'video',
    group: 'Capture troubleshooting',
    control: { kind: 'toggle' },
    default: true,
    advanced: true,
  },
  {
    key: 'gpu_compute_conversion',
    label: 'Copy and convert on a compute queue',
    description:
      'On supported AMD GPUs, copies captured frames and converts their colors on compute queues beside a game. Turn off to compare or troubleshoot.',
    details:
      'Covers direct driver push, Desktop Duplication and Windows Graphics Capture. Unsupported capture textures fall back to the graphics queue.',
    category: 'video',
    group: 'Capture troubleshooting',
    control: { kind: 'toggle' },
    default: true,
    advanced: true,
  },
  {
    key: 'experimental_packed_444',
    label: 'Experimental full chroma',
    description: 'Allow compatible Nightjar clients to request full-resolution color through HEVC or AV1 on AMD. Requires the client option and a host restart.',
    category: 'video',
    group: 'Codecs',
    control: { kind: 'toggle' },
    default: false,
    advanced: true,
  },
  {
    key: 'hevc_mode',
    label: 'HEVC support',
    description:
      'Which HEVC profiles devices may request. A profile the encoder fails when the host starts is never offered.',
    category: 'video',
    group: 'Codecs',
    control: {
      kind: 'select',
      options: [
        { value: '0', label: 'Automatic' },
        { value: '1', label: 'Off' },
        { value: '2', label: 'SDR only' },
        { value: '3', label: 'SDR and HDR' },
      ],
    },
    default: 0,
    restart: true,
  },
  {
    key: 'av1_mode',
    label: 'AV1 support',
    description:
      'Which AV1 profiles devices may request. A profile the encoder fails when the host starts is never offered.',
    category: 'video',
    group: 'Codecs',
    control: {
      kind: 'select',
      options: [
        { value: '0', label: 'Automatic' },
        { value: '1', label: 'Off' },
        { value: '2', label: 'SDR only' },
        { value: '3', label: 'SDR and HDR' },
      ],
    },
    default: 0,
    restart: true,
  },
  {
    key: 'pyrowave',
    label: 'PyroWave',
    description:
      'Lets compatible devices request PyroWave, a very low-latency GPU codec. It needs hundreds of Mbps and a fast wired network.',
    details:
      'PyroWave encodes each frame on its own. Synthetic desktop and game tests suggest 277 Mbps for 720p60, 399 Mbps for 1080p60 or 1593 Mbps for 4K60 for clean pictures. Severe detail loss is likely below 139, 187 or 747 Mbps respectively. Quality depends on the picture. Leave network headroom for packet overhead and recovery data; 4K60 needs more than gigabit Ethernet. If the device or network cannot carry the rate, use HEVC or AV1.',
    category: 'video',
    group: 'Codecs',
    control: { kind: 'toggle' },
    default: true,
    restart: true,
  },
  {
    key: 'prefer_sdr_10bit',
    label: 'Prefer 10-bit SDR',
    description:
      'When a device asks for HDR, keeps the display in standard dynamic range (SDR) and sends 10-bit SDR video. Reduces banding without switching to HDR.',
    details: 'Devices and apps can override it.',
    category: 'video',
    group: 'Codecs',
    control: { kind: 'toggle' },
    default: false,
  },
  {
    key: 'hdr_dynamic_metadata',
    label: 'HDR dynamic metadata',
    description:
      'Adds HDR10+ brightness statistics to HDR video so compatible displays can tone-map scene by scene. Devices without HDR10+ ignore them.',
    details:
      'HDR10+ goes into HEVC and AV1 streams. HEVC streams also carry HDR Vivid (CUVA 005.1) statistics. Moonlight drops them from HEVC before the decoder; Nightjar keeps them. In AV1 they reach any client whose decoder reads them. Turn this off if a display handles brightness worse with it.',
    category: 'video',
    group: 'Codecs',
    control: { kind: 'toggle' },
    default: true,
  },
  {
    key: 'hdr_dolby_vision',
    label: 'Dolby Vision (Experimental)',
    description:
      'Offers Dolby Vision profile 8.1, and profile 8.4 while HLG is on, to devices that ask for it on HEVC HDR streams. Experimental.',
    details:
      'Each frame carries its brightness statistics for the device’s Dolby Vision engine, and devices without Dolby Vision see HDR10 (8.1) or HLG (8.4). Packed 4:4:4 streams carry HDR10 and HDR10+ instead: the device unpacks the packed picture itself, so a Dolby Vision decoder would tone-map the packed layout instead of the picture.',
    category: 'video',
    group: 'Codecs',
    control: { kind: 'toggle' },
    default: true,
  },
  {
    key: 'hdr_hlg',
    label: 'HLG',
    description: 'Offers HLG HDR to devices that ask for it on HEVC and AV1 HDR streams.',
    details:
      'The picture is graded for a 1000-nit display, and the device’s screen tone-maps it from there. HEVC HLG streams carry HDR Vivid statistics while HDR dynamic metadata is on; HDR10+ only describes HDR10 streams. Packed 4:4:4 streams stay HDR10.',
    category: 'video',
    group: 'Codecs',
    control: { kind: 'toggle' },
    default: true,
  },
  {
    key: 'frame_pacing',
    label: 'Frame pacing',
    description:
      'On arrival encodes each new frame as soon as it is captured. Fixed grid encodes the newest frame at fixed intervals.',
    details:
      'On arrival never encodes faster than the stream rate. Variable refresh rate (VRR) streams ignore this setting.',
    category: 'video',
    group: 'Frame pacing',
    control: {
      kind: 'select',
      options: [
        { value: 'arrival', label: 'On arrival' },
        { value: 'grid', label: 'Fixed grid' },
      ],
    },
    aliases: { match: 'case', spellings: { fixed: 'grid' } },
    default: 'arrival',
  },
  {
    key: 'wgc_pacing_smoothing',
    label: 'Steady grid timing',
    description:
      'Keeps the fixed grid on a steady schedule. When off, each interval starts from the previous encode, so one late frame delays the ones after it.',
    category: 'video',
    group: 'Frame pacing',
    control: { kind: 'toggle' },
    default: true,
    inactiveHint: "Choose Fixed grid under Frame pacing to use this setting. VRR streams ignore fixed-grid pacing.",
    visibleWhen: gridPacing,
  },
  {
    key: 'minimum_fps_target',
    label: 'Still-picture resend rate',
    description:
      'How often an unchanged picture is sent again. Lower values save bandwidth on a still desktop; it does not limit a game’s frame rate.',
    details:
      'Each resend counts toward the stream frame rate, so a value close to it makes late game frames wait. 0 uses a fifth of the stream rate, at least 10; with PyroWave, 0 resends at the full stream rate. Other values are kept between 1 fps and the stream rate.',
    category: 'video',
    group: 'Frame pacing',
    control: { kind: 'number', min: 0, max: 1000, step: 0.1, unit: 'fps' },
    default: 20,
  },
  {
    key: 'limit_framerate',
    label: 'Use the launch frame rate',
    description:
      'Encodes at the frame rate the device asked for when it launched the app instead of the faster rate its stream request may ask for.',
    details:
      "Off, the stream runs at the stream request's rate, as in Vibepollo: Artemis's Warp frame pacing launches at 60 and asks for 120 or 240. The device's bitrate stays the stream's budget either way, so each frame gets less of it at the faster rate.",
    category: 'video',
    group: 'Frame pacing',
    control: { kind: 'toggle' },
    default: false,
  },
  {
    key: 'stream_reconfigure',
    label: 'Follow client size and frame rate changes',
    description:
      'Lets a Rubylight client change the stream resolution and frame rate without reconnecting, for example when a foldable phone opens its inner screen.',
    details:
      'The encoder restarts at the new size with a keyframe. The host display keeps its mode and the picture is scaled to fit. Clients that never ask see no change.',
    category: 'video',
    group: 'Frame pacing',
    control: { kind: 'toggle' },
    default: true,
  },
  {
    key: 'max_bitrate',
    label: 'Maximum bitrate',
    description: "Caps the bitrate a device can request. 0 uses the device's bitrate.",
    category: 'video',
    group: 'Bitrate and network',
    control: { kind: 'number', min: 0, step: 1, unit: 'Kbps' },
    default: 0,
  },
  {
    key: 'fec_percentage',
    label: 'Error correction',
    description:
      'Recovery packets sent with each video frame. Higher values survive more packet loss but leave less of the bitrate for the picture.',
    details: "Set as a share of each frame's data packets. PyroWave does not use it.",
    category: 'video',
    group: 'Bitrate and network',
    control: { kind: 'number', min: 0, max: 100, step: 1, unit: '%' },
    default: 20,
  },
  {
    key: 'host_adaptive_bitrate',
    label: 'Adapt bitrate on the host (experimental)',
    description:
      'Lowers the bitrate when a Moonlight device reports video loss, then slowly raises it again while traffic is sustained and latency is stable.',
    details:
      'Includes error correction and audio in the network budget. Requires observed client feedback; a device that adjusts its own bitrate takes control. H.264, HEVC and AV1 only. Changes apply on the next connection. It replaces Lower the bitrate when the link cannot carry it.',
    category: 'video',
    group: 'Bitrate and network',
    control: { kind: 'toggle' },
    default: false,
  },
  {
    key: 'adaptive_fec',
    label: 'Adapt error correction',
    description:
      'Uses less error correction while the device reports a clean link, and returns to the percentage above as soon as it reports loss.',
    details:
      'After loss, holds protection for 30 seconds, then reduces it by one percentage point every five seconds of clean reports. Devices that send no error-correction reports keep the configured percentage. Compatible PyroWave clients use the PyroWave error-correction setting as their ceiling.',
    category: 'video',
    group: 'Bitrate and network',
    control: { kind: 'toggle' },
    default: true,
  },
  {
    key: 'fec_max_percentage',
    label: 'Most error correction a device may ask for',
    description: 'The largest share of recovery packets a device may choose, as a percentage of the video packets.',
    details: 'Devices that choose their own error correction, or let the host follow their loss reports, stay at or below it. Others use Error correction above.',
    category: 'video',
    group: 'Bitrate and network',
    control: { kind: 'number', min: 0, max: 255, step: 1, unit: '%' },
    default: 50,
  },
  {
    key: 'pyrowave_critical_fec_percentage',
    label: 'PyroWave error correction',
    description:
      'Recovery packets that protect PyroWave pictures against packet loss.',
    details:
      'Newer clients protect the start of each frame, which holds its coarsest detail. Older clients use whole-frame protection within their packet limits. Oversized frames may receive less protection; a stream warning explains when. 0 turns it off.',
    category: 'video',
    group: 'Bitrate and network',
    control: { kind: 'number', min: 0, max: 255, step: 1, unit: '%' },
    default: 20,
    inactiveHint: "Turn on PyroWave to use this setting. Only compatible PyroWave clients use this error correction.",
    visibleWhen: (values) => flag(values, 'pyrowave', true),
  },
  {
    key: 'video_max_batch_size_kb',
    label: 'Send batch size',
    description:
      'Largest burst of video packets sent at once. Smaller batches can help devices that drop bursts.',
    details:
      'Smaller batches require more send calls. Conventional codecs also limit each batch to about one millisecond of traffic at the pacing rate. PyroWave uses its own batching policy.',
    category: 'video',
    group: 'Bitrate and network',
    control: {
      kind: 'select',
      options: [
        { value: '64', label: '64 KiB' },
        { value: '32', label: '32 KiB' },
        { value: '16', label: '16 KiB' },
      ],
    },
    default: 64,
  },
  {
    key: 'pacing_max_bitrate_kbps',
    label: 'Pacing rate',
    description:
      'Spaces out video packets without changing the picture bitrate. 0 paces automatically; reconnect after saving.',
    details:
      'Automatic pacing lets conventional codecs send up to 800 Mbps on wired routes, or twice the bitrate with FEC headroom on host Wi-Fi. PyroWave starts unmeasured paths near the stream budget and respects the client’s calibrated pace. Positive values are hard limits for PyroWave; conventional codecs retain a 1.1-times-bitrate floor. 120000 means 120 Mbps. A value here turns off Adaptive packet pacing and Slow down after video loss.',
    category: 'video',
    group: 'Bitrate and network',
    control: { kind: 'number', min: 0, step: 1, unit: 'Kbps' },
    default: 0,
    advanced: true,
  },
  {
    key: 'host_adaptive_pacing',
    label: 'Adaptive packet pacing (experimental)',
    description:
      'Starts with slower packet delivery, then tries faster rates and backs off when the device reports loss.',
    details:
      'Adds startup delay without changing picture bitrate or FPS; probing can still lose packets. Off by default, including for PyroWave. Changes apply on the next connection. A manual pacing rate or Adapt bitrate on the host turns it off.',
    category: 'video',
    group: 'Bitrate and network',
    control: { kind: 'toggle' },
    default: false,
    advanced: true,
  },
  {
    key: 'wireless_adaptive_pacing',
    label: 'Adaptive packet pacing for Wi-Fi clients',
    description:
      'When a client says its connection is Wi-Fi or cellular, paces its video adaptively from the first frame instead of after the first lost frames.',
    details:
      'Applies to H.264, HEVC and AV1 for clients that report a wireless route, such as Nightjar. Starts at twice the encoder bitrate, probes up in small steps while the link stays clean, up to the route limit, and backs off on loss, without changing picture bitrate or FPS. Slow down after video loss does not run on such a stream. PyroWave keeps its own pacing. Changes apply on the next connection. A manual pacing rate or Adapt bitrate on the host turns it off. With Adapt bitrate on the host or this setting off, H.264, HEVC and AV1 are still paced with the Wi-Fi route policy (twice the encoder bitrate, no probing) from the first frame; a manual pacing rate always wins.',
    category: 'video',
    group: 'Bitrate and network',
    control: { kind: 'toggle' },
    default: true,
    advanced: true,
  },
  {
    key: 'keyframe_loss_pacing',
    label: 'Slow down after video loss',
    description:
      'After a lost keyframe or repeated unrecoverable frames, spaces video packets more evenly for the rest of the stream without changing picture bitrate.',
    details:
      'Activates after a keyframe the client could not rebuild, a repeated request for the keyframe it was just sent, or three distinct unrecoverable frames within two seconds. Changes the host makes itself (Alt+Tab, a bitrate change, a restarted capture or encoder) do not count. Frames the host could not send whole, such as when the network send buffer fills at a high pacing rate, do: three within two seconds, or one keyframe. Loss that lasted is remembered for this device, address and network adapter for 24 hours, with a fast-rate retry every second eligible reconnect; a lost first keyframe alone is lifted after 10 clean seconds and not remembered. Reset remembered routes in Devices. Paces at twice the encoder bitrate, with room for error correction. Isolated ordinary-frame losses and fully repaired frames keep the fast rate. A manual pacing rate, Adaptive packet pacing or Adapt bitrate on the host turns it off. It also stays off on a stream that Adaptive packet pacing for Wi-Fi clients paces.',
    category: 'video',
    group: 'Bitrate and network',
    control: { kind: 'toggle' },
    default: true,
    advanced: true,
  },
  {
    key: 'link_rate_adaptation',
    label: 'Lower the bitrate when the link cannot carry it',
    description:
      'Cuts the picture bitrate when the network loses more than error correction can repair, then probes back up. Does nothing on a healthy link.',
    details:
      'Acts on three unrecoverable frames in two seconds, 5% of packets reported missing for two half-second windows, three frames lost on send, or three network keyframe requests from a client that sends no error-correction status. Each cut aims at 85% of what the link delivered, then the host probes back up in small steps and returns to the requested bitrate after ten clean seconds. Reports for frames sent before a cut took effect are ignored. Off for PyroWave. Adapt bitrate on the host wins when both are on, and a device that sets its own bitrate takes over bitrate control.',
    category: 'video',
    group: 'Bitrate and network',
    control: { kind: 'toggle' },
    default: true,
    advanced: true,
  },
];

const encoders: Setting[] = [
  {
    key: 'encoder',
    label: 'Encoder',
    description:
      "Automatic encodes H.264, HEVC and AV1 with AMD AMF, and PyroWave streams with PyroWave's own encoder.",
    details:
      "If AMF cannot start, the stream fails with AMF's error; there is no fallback to another encoder. The stream card and log show the encoder in use.",
    category: 'encoders',
    group: 'Encoder',
    control: {
      kind: 'select',
      options: [
        { value: 'auto', label: 'Automatic' },
        { value: 'amf', label: 'AMD AMF' },
        { value: 'pyrowave', label: 'PyroWave (automatic for other codecs)' },
      ],
    },
    // encoder_policy.rs canonical_name: Apollo and Vibepollo's names for AMF.
    aliases: {
      spellings: {
        amdvce: 'amf',
        amdvce_experimental: 'amf',
        amdvce_ffmpeg: 'amf',
        amdvce_legacy: 'amf',
      },
    },
    default: 'auto',
    restart: true,
  },
  {
    key: 'adapter_name',
    label: 'Graphics card',
    description:
      'The graphics processing unit (GPU) that captures and compresses the video. Automatic uses the card connected to the streamed display.',
    details:
      'Lists hardware GPUs even without a connected display. Available codecs depend on the GPU and driver.',
    category: 'encoders',
    group: 'Encoder',
    control: { kind: 'adapter' },
    default: '',
  },
  {
    key: 'adapter_pnp_id',
    label: 'Graphics card device ID',
    description:
      'Chooses the GPU by its Windows device instance ID instead of its name, for systems with two identical GPUs. When set, it takes precedence over the GPU name.',
    category: 'encoders',
    group: 'Encoder',
    control: { kind: 'text', placeholder: 'PCI\\VEN_1002&DEV_744C&...', mono: true },
    default: '',
    advanced: true,
  },
];

/** AMF properties the host can leave to the driver. */
const tristate = [
  { value: 'auto', label: 'Driver default' },
  { value: 'enabled', label: 'On' },
  { value: 'disabled', label: 'Off' },
];
/** encoder_policy.rs tristate: the on and off words, in any case; the fallback console writes true and false. */
const switchWords: Aliases = {
  match: 'case',
  spellings: {
    '1': 'enabled',
    true: 'enabled',
    yes: 'enabled',
    enable: 'enabled',
    on: 'enabled',
    '0': 'disabled',
    false: 'disabled',
    no: 'disabled',
    disable: 'disabled',
    off: 'disabled',
  },
};
/** encoder_policy.rs: alternative names for the entropy coders. */
const coderWords: Aliases = { spellings: { ac: 'cabac', vlc: 'cavlc' } };

const amd: Setting[] = [
  {
    key: 'amd_usage',
    label: 'Usage',
    description:
      'The base encoding profile; the settings below override parts of it. Profiles other than the low-latency ones add delay.',
    category: 'encoders',
    group: 'AMD AMF',
    control: {
      kind: 'select',
      options: [
        { value: 'ultralowlatency', label: 'Ultra low latency' },
        { value: 'lowlatency', label: 'Low latency' },
        { value: 'lowlatency_high_quality', label: 'Low latency, high quality' },
        { value: 'high_quality', label: 'High quality' },
        { value: 'webcam', label: 'Webcam' },
        { value: 'transcoding', label: 'Transcoding' },
        { value: 'auto', label: 'Driver default' },
      ],
    },
    default: 'ultralowlatency',
  },
  {
    key: 'amd_quality',
    label: 'Quality preset',
    description:
      'Trades encoding speed for picture quality. Driver default lets the usage profile decide.',
    category: 'encoders',
    group: 'AMD AMF',
    control: {
      kind: 'select',
      options: [
        { value: 'speed', label: 'Speed' },
        { value: 'balanced', label: 'Balanced' },
        { value: 'quality', label: 'Quality' },
        { value: 'auto', label: 'Driver default' },
      ],
    },
    default: 'speed',
  },
  {
    key: 'amd_rc',
    label: 'Rate control',
    description:
      'How the encoder keeps to the stream bitrate. Constant QP ignores the bitrate.',
    details:
      'The quality modes turn on pre-analysis, and HDR streams use peak-constrained VBR instead of them.',
    category: 'encoders',
    group: 'AMD AMF',
    control: {
      kind: 'select',
      options: [
        { value: 'vbr_latency', label: 'Latency-constrained VBR' },
        { value: 'vbr_peak', label: 'Peak-constrained VBR' },
        { value: 'cbr', label: 'Constant bitrate' },
        { value: 'qvbr', label: 'Quality VBR' },
        { value: 'hqvbr', label: 'High-quality VBR' },
        { value: 'hqcbr', label: 'High-quality CBR' },
        { value: 'cqp', label: 'Constant QP' },
        { value: 'auto', label: 'Driver default' },
      ],
    },
    default: 'vbr_latency',
  },
  {
    key: 'amd_peak_bitrate_ratio',
    label: 'Peak bitrate',
    description:
      'Peak bitrate as a multiple of the requested stream bitrate. 0 keeps the driver default.',
    details: 'This controls rate allocation; it does not cap individual keyframes.',
    category: 'encoders',
    group: 'AMD AMF',
    control: {
      kind: 'select',
      options: [
        { value: '0', label: 'Driver default' },
        { value: '1', label: '1× stream bitrate' },
        { value: '1.5', label: '1.5× stream bitrate' },
        { value: '2', label: '2× stream bitrate' },
      ],
    },
    default: 0,
    advanced: true,
  },
  {
    key: 'amd_vbv_buffer_frames',
    label: 'Rate-control buffer',
    description:
      'Rate-control buffer budget, in frames. Smaller budgets can reduce bursts at the cost of picture quality.',
    details: 'Frames are counted at the requested bitrate and frame rate. 0 keeps the driver default.',
    category: 'encoders',
    group: 'AMD AMF',
    control: {
      kind: 'select',
      options: [
        { value: '0', label: 'Driver default' },
        { value: '0.5', label: 'Half a frame' },
        { value: '1', label: 'One frame' },
        { value: '2', label: 'Two frames' },
      ],
    },
    default: 0,
    advanced: true,
  },
  {
    key: 'amd_max_frame_size',
    label: 'Maximum frame size',
    description:
      'Requests a size limit for every frame, including recovery keyframes. Tight limits can reduce picture quality.',
    details:
      'Automatic budgets about two frame periods at the current network pace, between 2× and 8× one frame’s bitrate budget. Faster links allow larger recovery frames. A fixed value overrides Automatic; 0 keeps the driver default. Limits are best-effort: frames can exceed them, and an unsupported limit warns and keeps streaming.',
    category: 'encoders',
    group: 'AMD AMF',
    control: {
      kind: 'select',
      options: [
        { value: 'auto', label: 'Automatic — follow network pace' },
        { value: '0', label: 'Driver default' },
        { value: '1', label: '1× frame budget' },
        { value: '2', label: '2× frame budget' },
        { value: '4', label: '4× frame budget' },
        { value: '8', label: '8× frame budget' },
      ],
    },
    default: 'auto',
    advanced: true,
  },
  {
    key: 'amd_qvbr_quality_level',
    label: 'Quality VBR level',
    description:
      'Target quality for quality VBR, from 1 (lowest) to 51 (highest). 0 keeps the driver default.',
    category: 'encoders',
    group: 'AMD AMF',
    control: { kind: 'number', min: 0, max: 51, step: 1 },
    default: 0,
    inactiveHint: "Applies with Rate control set to Quality VBR. HDR streams use peak-constrained VBR instead.",
    visibleWhen: (values) => read(values, 'amd_rc', 'vbr_latency') === 'qvbr',
  },
  {
    key: 'amd_preanalysis',
    label: 'Pre-analysis',
    description:
      'Analyzes each frame before encoding to improve rate control, at the cost of extra latency. Not used for HDR streams.',
    category: 'encoders',
    group: 'AMD AMF',
    control: { kind: 'toggle' },
    default: false,
  },
  {
    key: 'amd_vbaq',
    label: 'Variance-based adaptive quantization',
    description:
      'Gives more bits to smooth areas, where artifacts are easiest to see, and fewer to busy textures.',
    details: 'When unset, off for H.264 and on for HEVC/AV1. Not used with constant QP.',
    category: 'encoders',
    group: 'AMD AMF',
    control: {
      kind: 'select',
      options: [{ value: '', label: 'Default (H.264 off; HEVC/AV1 on)' }, ...tristate],
    },
    aliases: switchWords,
    default: '',
  },
  {
    key: 'amd_enforce_hrd',
    label: 'Enforce decoder buffer limits',
    description:
      'Holds rate control to the decoder buffer model, which greatly reduces bitrate spikes. Can cause artifacts or lower quality on some GPUs.',
    category: 'encoders',
    group: 'AMD AMF',
    control: { kind: 'toggle' },
    default: false,
  },
  {
    key: 'amd_coder',
    label: 'H.264 entropy coding',
    description: 'CABAC gives better quality per bit; CAVLC is simpler to decode. H.264 only.',
    category: 'encoders',
    group: 'AMD AMF',
    control: {
      kind: 'select',
      options: [
        { value: 'auto', label: 'Driver default' },
        { value: 'cabac', label: 'CABAC' },
        { value: 'cavlc', label: 'CAVLC' },
      ],
    },
    aliases: coderWords,
    default: 'auto',
  },
  {
    key: 'amd_input_queue_size',
    label: 'Input queue size',
    description:
      'How many frames may wait for the encoder. 0 uses the driver default, VRR streams included.',
    details:
      'The host sets a queue size only when you choose one: forcing a queue of one for every VRR stream exposed an RDNA4 driver freeze, with video stalling while audio plays.',
    category: 'encoders',
    group: 'AMD AMF',
    control: { kind: 'number', min: 0, max: 32, step: 1, unit: 'frames' },
    default: 0,
  },
  {
    key: 'amd_ltr_frames',
    label: 'Long-term reference frames',
    description:
      'Frames the encoder keeps as long-term references, so a client that lost packets recovers with a small frame instead of a full keyframe. AV1 only.',
    details: 'H.264 and HEVC always recover with a keyframe. 0 turns it off.',
    category: 'encoders',
    group: 'AMD AMF',
    control: { kind: 'number', min: 0, max: 4, step: 1, unit: 'frames' },
    default: 4,
  },
  {
    key: 'amd_lowlatency_mode',
    label: 'Low-latency mode',
    description:
      "Turns the encoder's internal low-latency mode on or off for H.264 and HEVC.",
    details:
      'The driver already turns it on for the Ultra low latency usage (the default) and Low latency, high quality; On changes only the other usages.',
    category: 'encoders',
    group: 'AMD AMF',
    control: { kind: 'select', options: tristate },
    aliases: switchWords,
    default: 'auto',
  },
  {
    key: 'amd_high_motion_quality_boost',
    label: 'High-motion quality boost',
    description: 'Improves quality in fast motion on drivers that support it.',
    category: 'encoders',
    group: 'AMD AMF',
    control: { kind: 'select', options: tristate },
    aliases: switchWords,
    default: 'auto',
  },
  {
    key: 'amd_smart_access_video',
    label: 'SmartAccess Video',
    description:
      'Lets the driver share encoding between AMD integrated and discrete graphics on systems that support SmartAccess Video.',
    category: 'encoders',
    group: 'AMD AMF',
    control: { kind: 'select', options: tristate },
    aliases: switchWords,
    default: 'auto',
  },
  {
    key: 'amd_split_frame',
    label: 'Split-frame encoding',
    description:
      'Lets the driver split HEVC or AV1 frames across two encoder engines. Leave Automatic unless comparing your own GPU.',
    details:
      'Automatic requests it only when the driver has it off; On requests it and Off disables the property. Measured RX 7900 XT throughput was similar across these choices. H.264 and GPUs with one engine ignore it.',
    category: 'encoders',
    group: 'AMD AMF',
    control: {
      kind: 'select',
      options: [
        { value: 'auto', label: 'Automatic' },
        { value: 'enabled', label: 'On' },
        { value: 'disabled', label: 'Off' },
      ],
    },
    aliases: switchWords,
    default: 'auto',
  },
  {
    key: 'encoder_focus',
    label: 'Encoder focus',
    description:
      'Gives more bits to parts of the picture that stay still, such as text and HUDs, and fewer to motion. Experimental.',
    details:
      'Off unless you choose a mode. The stream stays a normal video stream for every client. Still areas and centre also favors the middle of the screen and the cursor. Runs on H.264, HEVC and AV1 when the startup encoder check verified it on this GPU; otherwise the stream shows a warning and focus stays off.',
    category: 'encoders',
    group: 'AMD AMF',
    control: {
      kind: 'select',
      options: [
        { value: 'off', label: 'Off' },
        { value: 'static', label: 'Still areas' },
        { value: 'centre', label: 'Still areas and centre' },
      ],
    },
    aliases: { match: 'case', spellings: { center: 'centre' } },
    default: 'off',
  },
  {
    key: 'amd_av1_latency_mode',
    label: 'AV1 latency mode',
    description:
      'How quickly each AV1 frame is finished. Lower latency uses more power.',
    details:
      'The driver already picks the lowest latency for the Ultra low latency usage (the default) and Low latency, high quality.',
    category: 'encoders',
    group: 'AMD AMF',
    control: {
      kind: 'select',
      options: [
        { value: 'auto', label: 'Driver default' },
        { value: 'none', label: 'Balance latency and power' },
        { value: 'power_saving', label: 'Real time, power saving' },
        { value: 'realtime', label: 'Real time' },
        { value: 'lowest', label: 'Lowest latency' },
      ],
    },
    default: 'auto',
  },
  {
    key: 'amd_av1_screen_content',
    label: 'AV1 screen content tools',
    description:
      'AV1 coding tools for desktop content, which can make text and interface edges clearer. AV1 only.',
    category: 'encoders',
    group: 'AMD AMF',
    control: { kind: 'select', options: tristate },
    aliases: switchWords,
    default: 'auto',
  },
  {
    key: 'amd_av1_tiles',
    label: 'AV1 tiles',
    description: "Tiles per AV1 frame. Automatic follows the device's slice request, up to 4.",
    category: 'encoders',
    group: 'AMD AMF',
    control: {
      kind: 'select',
      options: [
        { value: '0', label: 'Automatic' },
        { value: '1', label: '1' },
        { value: '2', label: '2' },
        { value: '4', label: '4' },
      ],
    },
    default: 0,
    advanced: true,
  },
];

export const settings: Setting[] = [...video, ...encoders, ...amd];
