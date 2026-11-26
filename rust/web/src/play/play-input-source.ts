/**
 * Which input the play page was last used with, and the hints its glyph bar shows for it.
 *
 * A controller shows its own face buttons (Xbox letters, PlayStation shapes, Nintendo's swapped
 * letters), a keyboard its keys and a TV remote OK and Back. Touch and mouse show no hint: what
 * they can press is on the screen. Pure, so Play.test.mjs tests it in Node.
 */
import type { PadSnapshot } from '../lib/webrtc/gamepad';

export type PadFamily = 'xbox' | 'playstation' | 'nintendo';
export type InputSource = PadFamily | 'keyboard' | 'remote' | 'touch' | 'mouse';
/** Where the page is: what a press does differs. */
export type HintContext = 'pair-failed' | 'library' | 'stream' | 'dock' | 'error';

export interface Hint {
  /** What is printed on the button or key. */
  glyph: string;
  /** What pressing it does here. */
  label: string;
  /** Held for a second, not pressed. */
  hold?: boolean;
}

/** A pad's family from the name the browser gives it; an unknown pad draws as an Xbox one. */
export function padFamily(id: string): PadFamily {
  if (/054c|dualshock|dualsense|playstation|ps[345]\b/i.test(id)) return 'playstation';
  if (/057e|nintendo|joy-?con|switch/i.test(id)) return 'nintendo';
  return 'xbox';
}

const STICK = 0.5;

/** The family of the first pad with a button down or a stick moved; null when all are at rest. */
export function activePad(pads: readonly (PadSnapshot | null)[]): PadFamily | null {
  for (const pad of pads) {
    if (!pad) continue;
    if (pad.buttons.some((button) => button.pressed) || pad.axes.some((axis) => Math.abs(axis) > STICK)) return padFamily(pad.id);
  }
  return null;
}

/** TVs, consoles and cars: their browsers are driven by a remote, whatever keys it sends. */
const REMOTE_AGENT = /Web0S|webOS|Tizen|SMART-TV|SmartTV|\bTV\b|AFT|BRAVIA|Tesla|QtCarBrowser/i;
const REMOTE_KEYS = new Set(['GoBack', 'BrowserBack', 'XF86Back', 'MediaPlayPause', 'MediaRewind', 'MediaFastForward']);

/** A key press comes from a remote on a TV, and from a remote's own keys anywhere. */
export function keySource(event: { key: string; keyCode?: number }, agent: string): 'keyboard' | 'remote' {
  if (REMOTE_AGENT.test(agent) || REMOTE_KEYS.has(event.key) || event.keyCode === 461 || event.keyCode === 10009) return 'remote';
  return 'keyboard';
}

/** A pointer is a finger (or a pen) or a mouse. */
export const pointerSource = (pointerType: string): 'touch' | 'mouse' => (pointerType === 'mouse' ? 'mouse' : 'touch');

/** What the page assumes before anything was pressed: a remote on a TV, a finger on a phone. */
export function firstSource(agent: string, coarse: boolean): InputSource {
  if (/Xbox/i.test(agent)) return 'xbox';
  if (REMOTE_AGENT.test(agent)) return 'remote';
  return coarse ? 'touch' : 'mouse';
}

type Keys = { move: string; select: string; back: string; menu: string };

// The page reads a pad by position (the standard mapping): the bottom face button selects, the
// right one goes back, and the left centre button held opens the stream menu. Nintendo prints B
// on the bottom button and A on the right one.
const KEYS: Readonly<Record<Exclude<InputSource, 'touch' | 'mouse'>, Keys>> = {
  xbox: { move: 'D-pad', select: 'A', back: 'B', menu: 'View' },
  playstation: { move: 'D-pad', select: '×', back: '○', menu: 'Create' },
  nintendo: { move: 'D-pad', select: 'B', back: 'A', menu: '−' },
  keyboard: { move: 'Arrows', select: 'Enter', back: 'Esc', menu: 'Esc' },
  remote: { move: 'Arrows', select: 'OK', back: 'Back', menu: 'Back' },
};

/** The hints for `source` in `context`; none for touch and mouse. */
export function hints(source: InputSource, context: HintContext): Hint[] {
  if (source === 'touch' || source === 'mouse') return [];
  const keys = KEYS[source];
  switch (context) {
    case 'library':
      return [
        { glyph: keys.move, label: 'Move' },
        { glyph: keys.select, label: 'Play' },
      ];
    case 'stream':
      return [{ glyph: keys.menu, label: 'Menu', hold: true }];
    case 'dock':
      return [
        { glyph: keys.move, label: 'Move' },
        { glyph: keys.select, label: 'Choose' },
        { glyph: keys.back, label: 'Back to game' },
      ];
    case 'pair-failed':
      return [{ glyph: keys.select, label: 'New PIN' }];
    case 'error':
      return [{ glyph: keys.select, label: 'Try again' }];
  }
}

/** A hint read aloud: "Hold View: Menu". */
export const hintText = (hint: Hint): string => `${hint.hold ? 'Hold ' : ''}${hint.glyph}: ${hint.label}`;
