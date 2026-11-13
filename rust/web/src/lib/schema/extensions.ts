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
    label: '7.1.4 surround',
    description: 'Offers 7.1.4 surround sound to devices that ask for it; leave it on, since no other device is affected. Height channels stay silent unless the captured playback device has them.',
    category: 'audio',
    group: 'Sound',
    control: switches,
    default: true,
  },
  {
    key: 'pen_barrel_roll',
    label: 'Pen barrel roll',
    description: 'Passes on how far a stylus is turned around its own axis (barrel roll), for pens and devices that report it. Leave it on; it needs Native pen and touch.',
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
    label: 'Touch',
    description: "How this PC takes a device's touches: as sent, or as a mouse or trackpad for games that handle a mouse better than touch. Needs native pen and touch.",
    details:
      'As a mouse, the pointer follows the first finger and the left button is down while it touches. As a trackpad, the first finger moves the pointer by its swipe and a tap clicks left. With Native pen and touch off, the device turns touches into a mouse itself and this setting does nothing.',
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
    description: 'How far the pointer moves for a swipe as a trackpad, and how far two fingers scroll, in percent. Raise it if swipes move too little; lower it if the pointer overshoots.',
    category: 'input',
    group: 'Mouse, pen and touch',
    control: { kind: 'number', min: 25, max: 400, step: 1, unit: '%' },
    default: 100,
    visibleWhen: (values) => (values.touch_mode ?? 'client') !== 'client',
  },
  {
    key: 'touch_gestures',
    label: 'Touch gestures',
    description: 'As a mouse or trackpad, two fingers scroll and a two-finger tap clicks right. Leave it on unless a second finger must not end a drag: as a mouse it lets go of the left button.',
    category: 'input',
    group: 'Mouse, pen and touch',
    control: switches,
    default: true,
    visibleWhen: (values) => (values.touch_mode ?? 'client') !== 'client',
  },
  {
    key: 'client_cursor',
    label: 'Device-drawn cursor',
    description: 'Lets a device draw the mouse cursor itself, so it moves without waiting for the video. Leave it on; it needs Desktop Duplication capture, otherwise the cursor stays in the video.',
    category: 'input',
    group: 'Mouse, pen and touch',
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
    label: 'Pair with a QR code',
    description: 'Shows a QR code on Home and the Devices page that Nightjar scans to pair, instead of typing a PIN.',
    details: 'The code holds a one-time PIN and passphrase that work once and expire after 3 minutes, and the fingerprint of this PC\'s certificate, which Nightjar checks before pairing.',
    category: 'general',
    group: 'Pairing',
    control: switches,
    default: true,
    // Always on: the pairing page shows the code. `qr_pairing = false` in the file still turns it off.
    pending: true,
  },
  {
    key: 'dd_rotation_option',
    label: 'Rotate the display',
    description: "Match the device gives the streamed display the device's portrait or landscape shape, also as it turns, for portrait streams. A streamed monitor turns too, until the stream ends.",
    details:
      "Match the device makes a device's own virtual display in the stream's shape, and turns a monitor to it. A monitor turns back after the stream, or after a crash when Rubylight starts again, and needs Windows Graphics Capture while it is turned: Desktop Duplication refuses a turned monitor with an error.",
    category: 'display',
    group: 'Streamed display',
    control: { kind: 'select', options: adjust },
    default: 'disabled',
  },
  {
    key: 'dd_manual_rotation',
    label: 'Rotation',
    description: 'The angle the streamed display turns to, clockwise; a virtual display is made in that shape. A streamed monitor shows its picture turned by it until the stream ends.',
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
  // Nucleus Co-op background streaming and co-op, beside each game's Play together.
  {
    key: 'nucleus_path',
    label: 'Nucleus Co-op folder',
    description:
      "The folder you unpacked Nucleus Co-op 2.4.2 into, under C:\\Program Files; co-op and background streaming need it. Empty uses an earlier Rubylight's install, if any.",
    details: "Checked as it is saved, as on a game's Play together panel; a folder Nucleus Co-op can't run from is refused with the reason.",
    category: 'library',
    group: 'Play together',
    control: { kind: 'text', placeholder: 'C:\\Program Files\\Nucleus Co-op', mono: true, browse: 'directory' },
    default: '',
  },
  {
    key: 'nucleus_coop_players',
    label: 'Co-op players',
    description: "The most players a Nucleus co-op session gets, 1 to 4; each player runs their own instance of the game on this PC. The game's handler and the free controller slots can allow fewer.",
    category: 'library',
    group: 'Play together',
    control: { kind: 'number', min: 1, max: 4, step: 1 },
    default: 4,
  },
];
