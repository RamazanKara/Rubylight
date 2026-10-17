// Settings for the categories named in settings-schema.ts: video and encoders.
import type { ConfigValue } from '../api';
import { NATIVE_ENCODERS, type Aliases, type Setting } from '../settings-types';

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

/** The encoder the host resolves `encoder` to, including names from older hosts. */
function encoder(values: Values): string {
  const name = read(values, 'encoder', 'auto');
  switch (name) {
    case 'amdvce':
    case 'amdvce_experimental':
    case 'amdvce_ffmpeg':
    case 'amdvce_legacy':
      return 'amf';
    case 'nvenc_experimental':
      return 'nvenc';
    case 'amf':
    case 'nvenc':
    case 'nvenc_legacy':
    case 'quicksync':
    case 'qsv':
    case 'software':
      return name;
    default:
      return 'auto';
  }
}
/** Automatic uses this native encoder on one of this PC's GPUs. */
function automatic(values: Values, backend: string): boolean {
  const native = values[NATIVE_ENCODERS];
  return encoder(values) === 'auto' && Array.isArray(native) && native.includes(backend);
}
const nvenc = (values: Values) => ['nvenc', 'nvenc_legacy'].includes(encoder(values)) || automatic(values, 'nvenc');
const amf = (values: Values) => encoder(values) === 'amf' || automatic(values, 'amf');
const qsv = (values: Values) => ['qsv', 'quicksync'].includes(encoder(values));
const software = (values: Values) => encoder(values) === 'software';

const video: Setting[] = [
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
    group: 'Capture',
    control: { kind: 'number', min: 100, max: 1000, step: 50, unit: 'µs' },
    default: 500,
    advanced: true,
  },
  {
    key: 'idd_push_fence_ring',
    label: 'Experimental GPU fence transport',
    description:
      'Tries GPU fences for direct driver push on AMD. Requires a compatible Rubylight display driver and compute conversion.',
    details:
      'Off by default while performance is evaluated. Keeps the standard driver transport if unavailable. Physical displays and other capture methods are unaffected. End all streams before comparing.',
    category: 'video',
    group: 'Capture',
    control: { kind: 'toggle' },
    default: false,
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
    group: 'Capture',
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
    group: 'Capture',
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
    group: 'Capture',
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
    group: 'Capture',
    control: { kind: 'toggle' },
    default: true,
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
    label: 'Minimum frame rate',
    description:
      'How often an unchanged picture is sent again. Lower values save bandwidth on a still desktop.',
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
      'Includes error correction and audio in the network budget. Requires observed client feedback; a device that adjusts its own bitrate takes control. H.264, HEVC and AV1 only. Changes apply on the next connection.',
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
      'Devices that send no error-correction reports always get the percentage above. PyroWave does not use it.',
    category: 'video',
    group: 'Bitrate and network',
    control: { kind: 'toggle' },
    default: true,
  },
  {
    key: 'pyrowave_critical_fec_percentage',
    label: 'PyroWave error correction',
    description:
      'Recovery packets for the start of each PyroWave frame. Losing that part drops the whole frame.',
    details:
      'The start of a frame holds its coarsest detail. 0 turns it off, and only devices that support it use it.',
    category: 'video',
    group: 'Bitrate and network',
    control: { kind: 'number', min: 0, max: 255, step: 1, unit: '%' },
    default: 20,
    inactiveHint: "Turn on PyroWave to use this setting. Only compatible PyroWave clients use this error correction.",
    visibleWhen: (values) => flag(values, 'pyrowave', true),
  },
  {
    key: 'packetsize',
    label: 'Packet size',
    description:
      'Largest video packet the host sends, from 256 to 1400 bytes. 0 uses the size the device asks for; lower it if your network drops large packets.',
    category: 'video',
    group: 'Bitrate and network',
    control: { kind: 'number', min: 0, max: 1400, step: 1, unit: 'bytes' },
    default: 0,
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
      'Automatic pacing lets conventional codecs send up to 800 Mbps on wired routes, or twice the bitrate with FEC headroom on host Wi-Fi; PyroWave has its own automatic policy. A positive value also applies to PyroWave, with a 1.1-times-bitrate floor and an Ethernet link ceiling. 120000 means 120 Mbps.',
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
      'Adds startup delay without changing picture bitrate or FPS; probing can still lose packets. Off by default. A manual pacing rate, Host adaptive bitrate or PyroWave disables it. Changes apply on the next connection.',
    category: 'video',
    group: 'Bitrate and network',
    control: { kind: 'toggle' },
    default: false,
    advanced: true,
  },
  {
    key: 'keyframe_loss_pacing',
    label: 'Slow down after lost keyframes',
    description:
      'When a keyframe does not reach the device, sends video at twice the bitrate for the rest of the stream, as on Wi-Fi.',
    details:
      'A port slower than the host, such as a TV box with 100 Mbps Ethernet, can drop every keyframe sent at the fast wired rate, freezing the picture. Devices that never lose a keyframe keep the fast rate. A manual pacing rate, Adaptive packet pacing or Host adaptive bitrate disables it.',
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
      "Automatic picks AMF on AMD GPUs or NVENC on NVIDIA GPUs when the stream starts, and shows that encoder's settings below.",
    details:
      'If that encoder cannot start, Automatic tries another hardware encoder and shows a stream warning; it never falls back to software. Settings for other encoders appear when you select one, or in search. The stream card and log show the encoder actually used; PyroWave streams use their own encoder.',
    category: 'encoders',
    group: 'Encoder',
    control: {
      kind: 'select',
      options: [
        { value: 'auto', label: 'Automatic' },
        { value: 'amf', label: 'AMD AMF' },
        { value: 'nvenc', label: 'NVIDIA NVENC' },
        { value: 'nvenc_legacy', label: 'NVIDIA NVENC (FFmpeg)' },
        { value: 'quicksync', label: 'Intel Quick Sync' },
        { value: 'software', label: 'Software' },
        { value: 'pyrowave', label: 'PyroWave (automatic for other codecs)' },
      ],
    },
    // encoder_policy.rs canonical_name, and qsv, which the fallback console writes.
    aliases: {
      spellings: {
        qsv: 'quicksync',
        amdvce: 'amf',
        amdvce_experimental: 'amf',
        amdvce_ffmpeg: 'amf',
        amdvce_legacy: 'amf',
        nvenc_experimental: 'nvenc',
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
    control: { kind: 'text', placeholder: 'PCI\\VEN_10DE&DEV_2684&...', mono: true },
    default: '',
    advanced: true,
  },
];

const nvidia: Setting[] = [
  {
    key: 'nvenc_preset',
    label: 'Preset',
    description:
      'Higher presets compress better at the same bitrate but take longer to encode each frame. Raise it only when the network or the device limits the bitrate.',
    category: 'encoders',
    group: 'NVIDIA NVENC',
    control: {
      kind: 'select',
      options: [
        { value: '1', label: 'P1 (fastest)' },
        { value: '2', label: 'P2' },
        { value: '3', label: 'P3' },
        { value: '4', label: 'P4' },
        { value: '5', label: 'P5' },
        { value: '6', label: 'P6' },
        { value: '7', label: 'P7 (slowest)' },
      ],
    },
    default: 1,
    inactiveHint: "Applies to NVIDIA NVENC streams, including when Automatic selects NVENC. Shown when an NVENC encoder is selected, or with Automatic when this PC has an NVIDIA GPU.",
    visibleWhen: nvenc,
  },
  {
    key: 'nvenc_twopass',
    label: 'Multi-pass encoding',
    description:
      'A first pass over each frame spreads bits better and keeps frames within the bitrate. Turning it off can cause bitrate spikes and packet loss.',
    category: 'encoders',
    group: 'NVIDIA NVENC',
    control: {
      kind: 'select',
      options: [
        { value: 'disabled', label: 'Off' },
        { value: 'quarter_res', label: 'Quarter resolution' },
        { value: 'full_res', label: 'Full resolution' },
      ],
    },
    default: 'quarter_res',
    inactiveHint: "Applies to NVIDIA NVENC streams, including when Automatic selects NVENC. Shown when an NVENC encoder is selected, or with Automatic when this PC has an NVIDIA GPU.",
    visibleWhen: nvenc,
  },
  {
    key: 'nvenc_spatial_aq',
    label: 'Spatial adaptive quantization',
    description:
      'Gives flat areas of the picture fewer bits and detailed areas more. Can help at low bitrates.',
    category: 'encoders',
    group: 'NVIDIA NVENC',
    control: { kind: 'toggle' },
    default: false,
    inactiveHint: "Applies to NVIDIA NVENC streams, including when Automatic selects NVENC. Shown when an NVENC encoder is selected, or with Automatic when this PC has an NVIDIA GPU.",
    visibleWhen: nvenc,
  },
  {
    key: 'nvenc_temporal_aq',
    label: 'Temporal adaptive quantization',
    description:
      'Spends more bits on parts of the picture that stay the same across frames. Ignored on GPUs that do not support it.',
    category: 'encoders',
    group: 'NVIDIA NVENC',
    control: { kind: 'toggle' },
    default: false,
    inactiveHint: "Applies to NVIDIA NVENC streams, including when Automatic selects NVENC. Shown when an NVENC encoder is selected, or with Automatic when this PC has an NVIDIA GPU.",
    visibleWhen: nvenc,
  },
  {
    key: 'nvenc_vbv_increase',
    label: 'Frame size headroom',
    description:
      'Lets a single frame exceed the average frame size by this much. Helps busy scenes but can cause packet loss.',
    details:
      'Packet loss is a risk on networks without buffer headroom. 400 allows frames five times the average.',
    category: 'encoders',
    group: 'NVIDIA NVENC',
    control: { kind: 'number', min: 0, max: 400, step: 1, unit: '%' },
    default: 0,
    inactiveHint: "Applies to NVIDIA NVENC streams, including when Automatic selects NVENC. Shown when an NVENC encoder is selected, or with Automatic when this PC has an NVIDIA GPU.",
    visibleWhen: nvenc,
  },
  {
    key: 'nvenc_h264_cavlc',
    label: 'Use CAVLC for H.264',
    description:
      'Uses the simpler CAVLC entropy coding instead of CABAC. It needs about 10% more bitrate for the same quality and only very old decoders need it.',
    category: 'encoders',
    group: 'NVIDIA NVENC',
    control: { kind: 'toggle' },
    default: false,
    inactiveHint: "Applies to NVIDIA NVENC streams, including when Automatic selects NVENC. Shown when an NVENC encoder is selected, or with Automatic when this PC has an NVIDIA GPU.",
    visibleWhen: nvenc,
  },
  {
    key: 'nvenc_split_encode',
    label: 'Split-frame encoding',
    description:
      'Splits each HEVC or AV1 frame across several encoder engines on GPUs that have them. Automatic lets the driver decide, which it usually does at 4K and above.',
    category: 'encoders',
    group: 'NVIDIA NVENC',
    control: {
      kind: 'select',
      options: [
        { value: 'auto', label: 'Automatic' },
        { value: 'enabled', label: 'On' },
        { value: 'disabled', label: 'Off' },
      ],
    },
    // nvenc.rs: Vibepollo's nvenc_force_split_encode values and the fallback console's forced.
    aliases: { spellings: { true: 'enabled', forced: 'enabled', false: 'disabled', driver_decides: 'auto' } },
    default: 'auto',
    inactiveHint: "Applies to NVIDIA NVENC streams, including when Automatic selects NVENC. Shown when an NVENC encoder is selected, or with Automatic when this PC has an NVIDIA GPU.",
    visibleWhen: nvenc,
  },
  {
    key: 'nvenc_realtime_hags',
    label: 'Real-time graphics priority',
    description:
      'On NVIDIA GPUs with Windows GPU scheduling (HAGS) on, gives the host real-time GPU priority. Turn off if the encoder freezes when video memory is nearly full.',
    details:
      'Applies whenever an NVIDIA GPU captures the stream, whatever the encoder. HAGS is hardware-accelerated GPU scheduling, a Windows setting. When this setting is off, the host uses high priority instead.',
    category: 'encoders',
    // Not tied to the NVENC encoder: gpu_priority.rs checks the capture GPU's vendor.
    group: 'NVIDIA driver',
    control: { kind: 'toggle' },
    default: true,
    advanced: true,
  },
  {
    key: 'nvenc_latency_over_power',
    label: 'Prefer latency over power saving',
    description:
      'Sets the NVIDIA driver to prefer maximum performance for the host while streaming, so power saving does not slow encoding.',
    details:
      'Applies on every stream while an NVIDIA driver is installed, whatever the encoder. The driver setting is restored after the last stream.',
    category: 'encoders',
    // limiter.rs applies the driver preferences on every stream.
    group: 'NVIDIA driver',
    control: { kind: 'toggle' },
    default: true,
    advanced: true,
  },
  {
    key: 'nvenc_opengl_vulkan_on_dxgi',
    label: 'Present OpenGL and Vulkan through DXGI',
    description:
      'Sets the NVIDIA driver to present OpenGL and Vulkan games through DXGI while streaming, so full-screen games can be captured at full frame rate.',
    details:
      'Applies on every stream while an NVIDIA driver is installed, whatever the encoder. The driver setting is restored after the last stream.',
    category: 'encoders',
    group: 'NVIDIA driver',
    control: { kind: 'toggle' },
    default: true,
    advanced: true,
  },
  {
    key: 'nvenc_weighted_prediction',
    label: 'Weighted prediction',
    description:
      'Improves quality during fades and brightness changes on GPUs that support it, but turns off split-frame encoding for HEVC. Not used by NVENC (FFmpeg).',
    category: 'encoders',
    group: 'NVIDIA NVENC',
    control: { kind: 'toggle' },
    default: false,
    inactiveHint: "Applies to NVIDIA NVENC streams, including when Automatic selects NVENC. Shown when an NVENC encoder is selected, or with Automatic when this PC has an NVIDIA GPU.",
    visibleWhen: nvenc,
    advanced: true,
  },
  {
    key: 'nvenc_enable_min_qp',
    label: 'Minimum quantizer',
    description:
      'Stops the encoder from spending bits beyond a set quality level on easy scenes, leaving headroom for hard ones. Not used by NVENC (FFmpeg).',
    category: 'encoders',
    group: 'NVIDIA NVENC',
    control: { kind: 'toggle' },
    default: false,
    inactiveHint: "Applies to NVIDIA NVENC streams, including when Automatic selects NVENC. Shown when an NVENC encoder is selected, or with Automatic when this PC has an NVIDIA GPU.",
    visibleWhen: nvenc,
    advanced: true,
  },
  {
    key: 'nvenc_min_qp_h264',
    label: 'Minimum QP for H.264',
    description: 'The lowest quantization parameter (QP), which controls compression, for H.264 frames. Higher values cap quality sooner.',
    category: 'encoders',
    group: 'NVIDIA NVENC',
    control: { kind: 'number', min: 0, max: 51, step: 1 },
    default: 19,
    inactiveHint: "Applies to NVENC streams when Minimum quantizer is enabled, including when Automatic selects NVENC.",
    visibleWhen: (values) => nvenc(values) && flag(values, 'nvenc_enable_min_qp', false),
    advanced: true,
  },
  {
    key: 'nvenc_min_qp_hevc',
    label: 'Minimum QP for HEVC',
    description: 'The lowest quantization parameter (QP), which controls compression, for HEVC frames. Higher values cap quality sooner.',
    category: 'encoders',
    group: 'NVIDIA NVENC',
    control: { kind: 'number', min: 0, max: 51, step: 1 },
    default: 23,
    inactiveHint: "Applies to NVENC streams when Minimum quantizer is enabled, including when Automatic selects NVENC.",
    visibleWhen: (values) => nvenc(values) && flag(values, 'nvenc_enable_min_qp', false),
    advanced: true,
  },
  {
    key: 'nvenc_min_qp_av1',
    label: 'Minimum QP for AV1',
    description: 'The lowest quantization parameter (QP), which controls compression, for AV1 frames, from 0 to 255. Higher values cap quality sooner.',
    category: 'encoders',
    group: 'NVIDIA NVENC',
    control: { kind: 'number', min: 0, max: 255, step: 1 },
    default: 23,
    inactiveHint: "Applies to NVENC streams when Minimum quantizer is enabled, including when Automatic selects NVENC.",
    visibleWhen: (values) => nvenc(values) && flag(values, 'nvenc_enable_min_qp', false),
    advanced: true,
  },
  {
    key: 'nvenc_insert_filler_data',
    label: 'Insert filler data',
    description:
      'Pads frames so the stream holds a constant bitrate even when the picture is simple, which uses more bandwidth. Not used by NVENC (FFmpeg).',
    category: 'encoders',
    group: 'NVIDIA NVENC',
    control: { kind: 'toggle' },
    default: false,
    inactiveHint: "Applies to NVIDIA NVENC streams, including when Automatic selects NVENC. Shown when an NVENC encoder is selected, or with Automatic when this PC has an NVIDIA GPU.",
    visibleWhen: nvenc,
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
/** encoder_policy.rs: FFmpeg's names for the entropy coders. */
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
    inactiveHint: "Applies to AMD AMF streams, including when Automatic selects AMF. Shown when AMD AMF is selected, or with Automatic when this PC has an AMD GPU.",
    visibleWhen: amf,
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
    inactiveHint: "Applies to AMD AMF streams, including when Automatic selects AMF. Shown when AMD AMF is selected, or with Automatic when this PC has an AMD GPU.",
    visibleWhen: amf,
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
    inactiveHint: "Applies to AMD AMF streams, including when Automatic selects AMF. Shown when AMD AMF is selected, or with Automatic when this PC has an AMD GPU.",
    visibleWhen: amf,
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
    inactiveHint: "Applies to AMD AMF streams, including when Automatic selects AMF. Shown when AMD AMF is selected, or with Automatic when this PC has an AMD GPU.",
    visibleWhen: amf,
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
    inactiveHint: "Applies to AMD AMF streams, including when Automatic selects AMF. Shown when AMD AMF is selected, or with Automatic when this PC has an AMD GPU.",
    visibleWhen: amf,
    advanced: true,
  },
  {
    key: 'amd_max_frame_size',
    label: 'Maximum frame size',
    description:
      'Requests a size limit for every frame, including recovery keyframes. Tight limits can reduce picture quality.',
    details:
      'Defaults to 2× one frame’s bitrate budget to reduce large recovery bursts. 0 keeps the driver default. Limits are best-effort: frames can exceed them, and an unsupported limit warns and keeps streaming.',
    category: 'encoders',
    group: 'AMD AMF',
    control: {
      kind: 'select',
      options: [
        { value: '0', label: 'Driver default' },
        { value: '1', label: '1× frame budget' },
        { value: '2', label: '2× frame budget' },
        { value: '4', label: '4× frame budget' },
        { value: '8', label: '8× frame budget' },
      ],
    },
    default: 2,
    inactiveHint: "Applies to AMD AMF streams, including when Automatic selects AMF. Shown when AMD AMF is selected, or with Automatic when this PC has an AMD GPU.",
    visibleWhen: amf,
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
    inactiveHint: "Applies to AMD AMF with Rate control set to Quality VBR. HDR streams use peak-constrained VBR instead.",
    visibleWhen: (values) => amf(values) && read(values, 'amd_rc', 'vbr_latency') === 'qvbr',
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
    inactiveHint: "Applies to AMD AMF streams, including when Automatic selects AMF. Shown when AMD AMF is selected, or with Automatic when this PC has an AMD GPU.",
    visibleWhen: amf,
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
    inactiveHint: "Applies to AMD AMF streams, including when Automatic selects AMF. Shown when AMD AMF is selected, or with Automatic when this PC has an AMD GPU.",
    visibleWhen: amf,
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
    inactiveHint: "Applies to AMD AMF streams, including when Automatic selects AMF. Shown when AMD AMF is selected, or with Automatic when this PC has an AMD GPU.",
    visibleWhen: amf,
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
    inactiveHint: "Applies to AMD AMF streams, including when Automatic selects AMF. Shown when AMD AMF is selected, or with Automatic when this PC has an AMD GPU.",
    visibleWhen: amf,
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
    inactiveHint: "Applies to AMD AMF streams, including when Automatic selects AMF. Shown when AMD AMF is selected, or with Automatic when this PC has an AMD GPU.",
    visibleWhen: amf,
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
    inactiveHint: "Applies to AMD AMF streams, including when Automatic selects AMF. Shown when AMD AMF is selected, or with Automatic when this PC has an AMD GPU.",
    visibleWhen: amf,
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
    inactiveHint: "Applies to AMD AMF streams, including when Automatic selects AMF. Shown when AMD AMF is selected, or with Automatic when this PC has an AMD GPU.",
    visibleWhen: amf,
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
    inactiveHint: "Applies to AMD AMF streams, including when Automatic selects AMF. Shown when AMD AMF is selected, or with Automatic when this PC has an AMD GPU.",
    visibleWhen: amf,
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
    inactiveHint: "Applies to AMD AMF streams, including when Automatic selects AMF. Shown when AMD AMF is selected, or with Automatic when this PC has an AMD GPU.",
    visibleWhen: amf,
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
    inactiveHint: "Applies to AMD AMF streams, including when Automatic selects AMF. Shown when AMD AMF is selected, or with Automatic when this PC has an AMD GPU.",
    visibleWhen: amf,
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
    inactiveHint: "Applies to AMD AMF streams, including when Automatic selects AMF. Shown when AMD AMF is selected, or with Automatic when this PC has an AMD GPU.",
    visibleWhen: amf,
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
    inactiveHint: "Applies to AMD AMF streams, including when Automatic selects AMF. Shown when AMD AMF is selected, or with Automatic when this PC has an AMD GPU.",
    visibleWhen: amf,
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
    inactiveHint: "Applies to AMD AMF streams, including when Automatic selects AMF. Shown when AMD AMF is selected, or with Automatic when this PC has an AMD GPU.",
    visibleWhen: amf,
    advanced: true,
  },
];

const intel: Setting[] = [
  {
    key: 'qsv_preset',
    label: 'Preset',
    description:
      'Slower presets give better quality at the same bitrate and take longer to encode each frame.',
    category: 'encoders',
    group: 'Intel Quick Sync',
    control: {
      kind: 'select',
      options: [
        { value: 'veryfast', label: 'Very fast' },
        { value: 'faster', label: 'Faster' },
        { value: 'fast', label: 'Fast' },
        { value: 'medium', label: 'Medium' },
        { value: 'slow', label: 'Slow' },
        { value: 'slower', label: 'Slower' },
        { value: 'veryslow', label: 'Very slow' },
      ],
    },
    default: 'medium',
    inactiveHint: "Applies to Intel Quick Sync streams, including automatic hardware fallback. Select Intel Quick Sync to show this group outside search.",
    visibleWhen: qsv,
  },
  {
    key: 'qsv_coder',
    label: 'H.264 entropy coding',
    description: 'CABAC gives better quality per bit; CAVLC is simpler to decode. H.264 only.',
    category: 'encoders',
    group: 'Intel Quick Sync',
    control: {
      kind: 'select',
      options: [
        { value: 'auto', label: 'Automatic' },
        { value: 'cabac', label: 'CABAC' },
        { value: 'cavlc', label: 'CAVLC' },
      ],
    },
    aliases: coderWords,
    default: 'auto',
    inactiveHint: "Applies to Intel Quick Sync streams, including automatic hardware fallback. Select Intel Quick Sync to show this group outside search.",
    visibleWhen: qsv,
  },
  {
    key: 'qsv_slow_hevc',
    label: 'Allow slow HEVC encoding',
    description:
      'Uses the general-purpose HEVC encoder instead of the low-power one. This enables HEVC on older Intel GPUs at the cost of higher GPU load.',
    category: 'encoders',
    group: 'Intel Quick Sync',
    control: { kind: 'toggle' },
    default: false,
    inactiveHint: "Applies to Intel Quick Sync streams, including automatic hardware fallback. Select Intel Quick Sync to show this group outside search.",
    visibleWhen: qsv,
  },
];

const cpu: Setting[] = [
  {
    key: 'sw_preset',
    label: 'Preset',
    description:
      'Faster presets use less CPU; slower ones give better quality at the same bitrate. Applies to H.264, HEVC and AV1.',
    category: 'encoders',
    group: 'Software',
    control: {
      kind: 'select',
      options: [
        { value: 'ultrafast', label: 'Ultra fast' },
        { value: 'superfast', label: 'Super fast' },
        { value: 'veryfast', label: 'Very fast' },
        { value: 'faster', label: 'Faster' },
        { value: 'fast', label: 'Fast' },
        { value: 'medium', label: 'Medium' },
        { value: 'slow', label: 'Slow' },
        { value: 'slower', label: 'Slower' },
        { value: 'veryslow', label: 'Very slow' },
      ],
    },
    default: 'superfast',
    inactiveHint: "Choose Software under Encoder to use this setting. Automatic never selects software encoding.",
    visibleWhen: software,
  },
  {
    key: 'sw_tune',
    label: 'Tuning',
    description:
      'Adjusts the H.264 and HEVC encoders for a kind of content. Zero latency suits streaming; film and still image work with H.264 only.',
    category: 'encoders',
    group: 'Software',
    control: {
      kind: 'select',
      options: [
        { value: 'zerolatency', label: 'Zero latency' },
        { value: 'film', label: 'Film' },
        { value: 'animation', label: 'Animation' },
        { value: 'grain', label: 'Grain' },
        { value: 'stillimage', label: 'Still image' },
        { value: 'fastdecode', label: 'Fast decode' },
      ],
    },
    default: 'zerolatency',
    inactiveHint: "Choose Software under Encoder to use this setting. Automatic never selects software encoding.",
    visibleWhen: software,
  },
  {
    key: 'min_threads',
    label: 'CPU threads',
    description:
      'Threads the software encoder uses. More threads cost a little compression efficiency; use the lowest number that keeps up with your stream.',
    category: 'encoders',
    group: 'Software',
    control: { kind: 'number', min: 1, max: 64, step: 1 },
    default: 2,
    inactiveHint: "Choose Software under Encoder to use this setting. Automatic never selects software encoding.",
    visibleWhen: software,
  },
];

export const settings: Setting[] = [...video, ...encoders, ...nvidia, ...amd, ...intel, ...cpu];
