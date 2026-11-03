// Gamepads for the guest page, on the `control` and `gamepad` channels of the host's offer
// (docs/webrtc.md, "Gamepads"): the Gamepad API's standard buttons in the TV app's bit order, as
// rust/core/src/webrtc/gamepad.rs reads them. The mapping is pure, so it is tested in Node
// (Guests.test.mjs).

export type PadSnapshot = {
  index: number;
  id: string;
  mapping: string;
  buttons: readonly { pressed: boolean; value: number }[];
  axes: readonly number[];
  vibrationActuator?: unknown;
};

export type PadMessage = { v: 2; type: string; controllerId: number; [field: string]: unknown };

/** Gamepad API button index and its bit in `gamepad-state`. Indices 6 and 7 are the triggers. */
const BUTTON_BITS: readonly [number, number][] = [
  [0, 0], // A
  [1, 1], // B
  [2, 2], // X
  [3, 3], // Y
  [4, 4], // left bumper
  [5, 5], // right bumper
  [8, 6], // back
  [9, 7], // start
  [10, 8], // left stick
  [11, 9], // right stick
  [12, 10], // d-pad up
  [13, 11], // d-pad down
  [14, 12], // d-pad left
  [15, 13], // d-pad right
  [16, 14], // guide
];

/** An unchanged state is sent again this often, as the TV app does. */
export const KEEPALIVE_MS = 250;

export function connectedMessage(pad: PadSnapshot): PadMessage {
  return {
    v: 2,
    type: 'gamepad-connected',
    controllerId: pad.index,
    id: pad.id.slice(0, 512),
    mapping: pad.mapping.slice(0, 128),
    buttons: Math.min(pad.buttons.length, 64),
    axes: Math.min(pad.axes.length, 16),
    dualRumble: pad.vibrationActuator != null,
    triggerRumble: false,
  };
}

export function stateMessage(pad: PadSnapshot, seq: number): PadMessage {
  const value = (index: number) => pad.buttons[index]?.value ?? 0;
  const axis = (index: number) => pad.axes[index] ?? 0;
  const buttons = BUTTON_BITS.reduce((mask, [index, bit]) => (pad.buttons[index]?.pressed ? mask | (1 << bit) : mask), 0);
  return {
    v: 2,
    type: 'gamepad-state',
    controllerId: pad.index,
    seq,
    buttons,
    leftTrigger: value(6),
    rightTrigger: value(7),
    leftStickX: axis(0),
    leftStickY: axis(1),
    rightStickX: axis(2),
    rightStickY: axis(3),
  };
}

/**
 * The pads the page has announced and what it sent for each. `poll` takes `getGamepads()` and
 * says what to send: announcements and unplugs on `control`, changed or due states on `gamepad`.
 */
export class Pads {
  private known = new Map<number, { id: string; state: string; at: number }>();
  private seq = 0;

  poll(pads: readonly (PadSnapshot | null)[], now: number): { control: PadMessage[]; gamepad: PadMessage[] } {
    const control: PadMessage[] = [];
    const gamepad: PadMessage[] = [];
    const present = new Set<number>();
    for (const pad of pads) {
      if (!pad) continue;
      present.add(pad.index);
      let known = this.known.get(pad.index);
      if (known && known.id !== pad.id) {
        control.push({ v: 2, type: 'gamepad-disconnected', controllerId: pad.index });
        known = undefined;
      }
      if (!known) {
        control.push(connectedMessage(pad));
        known = { id: pad.id, state: '', at: -Infinity };
        this.known.set(pad.index, known);
      }
      const state = stateMessage(pad, 0);
      const key = JSON.stringify(state);
      if (key !== known.state || now - known.at >= KEEPALIVE_MS) {
        gamepad.push({ ...state, seq: ++this.seq });
        known.state = key;
        known.at = now;
      }
    }
    for (const index of [...this.known.keys()]) {
      if (present.has(index)) continue;
      this.known.delete(index);
      control.push({ v: 2, type: 'gamepad-disconnected', controllerId: index });
    }
    return { control, gamepad };
  }

  /** Unplugs every announced pad, when the page stops sending. */
  release(): PadMessage[] {
    const out = [...this.known.keys()].map((controllerId) => ({ v: 2, type: 'gamepad-disconnected', controllerId }) as const);
    this.known.clear();
    return out;
  }
}
