// The settings of the 3.0 extensions, defined ahead of their features. Each
// is `pending` and stays out of the console until its workstream ships it;
// the host reads them in core/src/config/settings30.rs, with these defaults.
// When a setting goes live, its label follows the release rule: features
// ported from Foundation Sunshine, where they are established, ship as normal
// features (credited in the code, NOTICE and AGENTS.md, never in the console
// or release notes), and new work that our devices can't prove is labelled
// Experimental.
import type { Setting } from '../settings-types';

const switches = { kind: 'toggle' } as const;
const adjust = [
  { value: 'disabled', label: 'Leave as it is' },
  { value: 'auto', label: 'Match the device' },
  { value: 'manual', label: 'Choose below' },
];

export const settings: Setting[] = [
  {
    key: 'audio_surround_714',
    label: '7.1.4 surround (Experimental)',
    description: 'Offers 7.1.4 surround sound to devices that ask for it. Height channels stay silent unless the captured playback device has them.',
    category: 'audio',
    control: switches,
    default: true,
  },
  {
    key: 'pen_barrel_roll',
    label: 'Pen barrel roll',
    description: 'Passes on how far a stylus is turned around its own axis, for pens that report it.',
    category: 'input',
    group: 'Mouse, pen and touch',
    control: switches,
    default: true,
    pending: true,
  },
  {
    key: 'touch_keyboard_digitizer',
    label: 'Windows touch keyboard',
    description: "Lets the Windows touch keyboard open for a device's touches.",
    category: 'input',
    group: 'Keyboard',
    control: switches,
    default: false,
    pending: true,
  },
  {
    key: 'touch_mode',
    label: 'Touch',
    description: 'How this PC takes touches: as the device sends them, as a mouse or as a trackpad.',
    category: 'input',
    group: 'Mouse, pen and touch',
    control: {
      kind: 'select',
      options: [
        { value: 'client', label: 'As the device sends them' },
        { value: 'mouse', label: 'Mouse' },
        { value: 'trackpad', label: 'Trackpad' },
      ],
    },
    default: 'client',
    pending: true,
  },
  {
    key: 'touch_sensitivity',
    label: 'Touch sensitivity',
    description: 'How far the pointer moves for a swipe as a mouse or trackpad.',
    category: 'input',
    control: { kind: 'number', min: 25, max: 400, step: 1, unit: '%' },
    default: 100,
    pending: true,
  },
  {
    key: 'touch_gestures',
    label: 'Touch gestures',
    description: 'Turns touch gestures with more than one finger into scrolling and clicks.',
    category: 'input',
    control: switches,
    default: true,
    pending: true,
  },
  {
    key: 'client_cursor',
    label: 'Device-drawn cursor',
    description: 'Lets a device draw the mouse cursor itself, so the cursor moves without waiting for the video.',
    category: 'input',
    control: switches,
    default: true,
    pending: true,
  },
  {
    key: 'webhooks',
    label: 'Webhooks',
    description: 'Addresses this PC notifies when devices pair, apps start and stop, and streams begin and end.',
    category: 'general',
    group: 'Webhooks',
    control: { kind: 'text', placeholder: '[{"url": "https://…", "events": ["app.started"], "secret": "…"}]', mono: true },
    default: '[]',
    pending: true,
  },
  {
    key: 'qr_pairing',
    label: 'Pair with a QR code',
    description: 'Shows a QR code that a device scans to pair, instead of typing a PIN.',
    category: 'general',
    group: 'Pairing',
    control: switches,
    default: false,
    pending: true,
  },
  {
    key: 'dd_rotation_option',
    label: 'Rotate the display (Experimental)',
    description: "Whether the streamed display takes the device's portrait or landscape shape, and follows it when the device turns. Experimental.",
    details:
      "Match the device makes a device's own virtual display in the stream's shape, and turns a monitor to it. A monitor turns back after the stream, or after a crash when the host starts again, and needs Windows Graphics Capture while it is turned.",
    category: 'display',
    group: 'Streamed display',
    control: { kind: 'select', options: adjust },
    default: 'disabled',
  },
  {
    key: 'dd_manual_rotation',
    label: 'Rotation (Experimental)',
    description: 'The angle the streamed display turns to, clockwise; a virtual display is made in that shape. Experimental.',
    category: 'display',
    group: 'Streamed display',
    control: {
      kind: 'select',
      options: [0, 90, 180, 270].map((degrees) => ({ value: String(degrees), label: `${degrees}°` })),
    },
    default: 0,
    inactiveHint: 'Set Rotate the display to Choose below to apply this angle.',
    visibleWhen: (values) => String(values.dd_rotation_option ?? '').trim() === 'manual',
  },
  {
    key: 'background_mode',
    label: 'Background streaming',
    description: 'Where a stream runs beside the signed-in user: chosen automatically, in a separate Windows session or on a virtual display.',
    category: 'general',
    group: 'Background streaming',
    control: {
      kind: 'select',
      options: [
        { value: 'auto', label: 'Automatic' },
        { value: 'child_session', label: 'Separate Windows session' },
        { value: 'virtual_display', label: 'Virtual display' },
      ],
    },
    default: 'auto',
    pending: true,
  },
  {
    key: 'child_session_resolution',
    label: 'Session resolution',
    description: 'The resolution of the separate Windows session.',
    category: 'general',
    group: 'Background streaming',
    control: { kind: 'text', placeholder: '1920x1080', mono: true },
    default: '1920x1080',
    pending: true,
  },
  {
    key: 'child_session_scale',
    label: 'Session scale',
    description: 'The scaling of the separate Windows session.',
    category: 'general',
    group: 'Background streaming',
    control: { kind: 'number', min: 100, max: 500, step: 1, unit: '%' },
    default: 100,
    pending: true,
  },
  {
    key: 'child_session_port',
    label: 'Session port',
    description: "The base port of the separate session's streams, which need ports of their own.",
    category: 'general',
    group: 'Background streaming',
    control: { kind: 'number', min: 1029, max: 65514, step: 1 },
    default: 48289,
    pending: true,
  },
];
