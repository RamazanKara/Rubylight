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
    label: 'Pen barrel roll (Experimental)',
    description: 'Passes on how far a stylus is turned around its own axis, for pens and devices that report it.',
    category: 'input',
    group: 'Mouse, pen and touch',
    control: switches,
    default: true,
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
    label: 'Touch (Experimental)',
    description: 'How this PC takes touches: as the device sends them, as a mouse or as a trackpad. Needs native pen and touch.',
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
  },
  {
    key: 'touch_sensitivity',
    label: 'Touch sensitivity',
    description: 'How far the pointer moves for a swipe as a trackpad, and how far two fingers scroll.',
    category: 'input',
    control: { kind: 'number', min: 25, max: 400, step: 1, unit: '%' },
    default: 100,
    visibleWhen: (values) => (values.touch_mode ?? 'client') !== 'client',
  },
  {
    key: 'touch_gestures',
    label: 'Touch gestures',
    description: 'As a mouse or trackpad, two fingers scroll and a two-finger tap clicks right.',
    category: 'input',
    control: switches,
    default: true,
    visibleWhen: (values) => (values.touch_mode ?? 'client') !== 'client',
  },
  {
    key: 'client_cursor',
    label: 'Device-drawn cursor (Experimental)',
    description: 'Lets a device draw the mouse cursor itself, so the cursor moves without waiting for the video. Needs Desktop Duplication capture; with other capture the cursor stays in the video.',
    category: 'input',
    control: switches,
    default: true,
  },
  {
    key: 'webhooks',
    label: 'Webhooks',
    description: 'Addresses this PC notifies when devices pair, apps start and stop, and streams begin and end.',
    category: 'general',
    group: 'Webhooks',
    control: { kind: 'text', placeholder: '[{"url": "https://…", "events": ["app.started"], "secret": "…"}]', mono: true },
    default: '[]',
    // The Webhooks page manages this list and keeps its secrets: never a setting field.
    pending: true,
  },
  {
    key: 'qr_pairing',
    label: 'Pair with a QR code (Experimental)',
    description: 'Shows a QR code on the Devices page that Nightjar scans to pair, instead of typing a PIN. Experimental.',
    details: 'The code holds a one-time PIN and passphrase that work once and expire after 3 minutes, and the fingerprint of the host certificate, which Nightjar checks before pairing.',
    category: 'general',
    group: 'Pairing',
    control: switches,
    default: false,
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
    key: 'child_session_port',
    label: 'Session port',
    description: "The base port of the separate session's streams, which need ports of their own.",
    category: 'general',
    group: 'Background streaming',
    control: { kind: 'number', min: 1029, max: 65514, step: 1 },
    default: 48289,
    pending: true,
  },
  // Nucleus Co-op background streaming and co-op.
  {
    key: 'nucleus_path',
    label: 'Nucleus Co-op folder',
    description: "The folder of your own Nucleus Co-op copy. Empty: the host's own copy, which it downloads on first use.",
    category: 'general',
    group: 'Background streaming',
    control: { kind: 'text', placeholder: 'C:\\Tools\\Nucleus Co-op', mono: true },
    default: '',
    pending: true,
  },
  {
    key: 'nucleus_coop_players',
    label: 'Co-op players',
    description: "The most players a Nucleus co-op session gets. The game's handler and the free controller slots can allow fewer.",
    category: 'general',
    group: 'Background streaming',
    control: { kind: 'number', min: 1, max: 4, step: 1 },
    default: 4,
    pending: true,
  },
];
