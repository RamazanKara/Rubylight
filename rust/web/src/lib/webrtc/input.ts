// Keyboard and mouse for the Stream page, on the client-opened `input` DataChannel
// (docs/webrtc.md, "The input channel"). Ported from rust/tests/webrtc_client/input/input.js.
// The mapping is pure, so it is tested in Node (WebrtcStream.test.mjs).

export type InputMessage =
  | { v: 1; type: 'key'; code: string; down: boolean }
  | { v: 1; type: 'mouse-position'; x: number; y: number }
  | { v: 1; type: 'mouse-button'; button: number; down: boolean }
  | { v: 1; type: 'wheel'; dx: number; dy: number };

/** The label and options the host accepts for keyboard and mouse: ordered and reliable. */
export const INPUT_LABEL = 'input';
export const INPUT_OPTIONS: RTCDataChannelInit = { ordered: true };

/** A key by its DOM `code`, so the layout is the host's. Events without a code are dropped. */
export function keyMessage(code: string, down: boolean): InputMessage | null {
  return code ? { v: 1, type: 'key', code, down } : null;
}

/** DOM buttons 0 to 4: left, middle, right, back, forward. */
export function buttonMessage(button: number, down: boolean): InputMessage | null {
  return Number.isInteger(button) && button >= 0 && button <= 4 ? { v: 1, type: 'mouse-button', button, down } : null;
}

const clamp = (value: number) => Math.min(1, Math.max(0, value));

/**
 * Where a pointer at (clientX, clientY) is on the picture, as a fraction of the video: the
 * element letterboxes the video (object-fit: contain), so the bars are left out. Null before
 * the first frame, when the size is unknown.
 */
export function positionMessage(
  clientX: number,
  clientY: number,
  box: { left: number; top: number; width: number; height: number },
  video: { width: number; height: number },
): InputMessage | null {
  if (!video.width || !video.height || !box.width || !box.height) return null;
  const scale = Math.min(box.width / video.width, box.height / video.height);
  const width = video.width * scale;
  const height = video.height * scale;
  const left = box.left + (box.width - width) / 2;
  const top = box.top + (box.height - height) / 2;
  return { v: 1, type: 'mouse-position', x: clamp((clientX - left) / width), y: clamp((clientY - top) / height) };
}

/** WheelEvent.deltaMode: pixels, lines, pages. */
const PIXEL = 0;
const LINE = 1;
/** One notch is 120 units; Chromium scrolls 100 pixels and Firefox 3 lines per notch. */
const UNITS_PER_PIXEL = 1.2;
const UNITS_PER_LINE = 40;
const UNITS_PER_PAGE = 360;

/** A wheel event in 120 units per notch with DOM signs (positive `dy` scrolls down). */
export function wheelMessage(deltaX: number, deltaY: number, deltaMode: number): InputMessage | null {
  const unit = deltaMode === PIXEL ? UNITS_PER_PIXEL : deltaMode === LINE ? UNITS_PER_LINE : UNITS_PER_PAGE;
  const dx = Math.round(deltaX * unit);
  const dy = Math.round(deltaY * unit);
  return dx || dy ? { v: 1, type: 'wheel', dx, dy } : null;
}

/**
 * The keys and buttons the host holds for this page. A browser that loses the focus never
 * sees their release, so `releaseAll` lets them go on the host.
 */
export class Held {
  private keys = new Set<string>();
  private buttons = new Set<number>();

  /** Records a message before it is sent. */
  track(message: InputMessage) {
    if (message.type === 'key') {
      if (message.down) this.keys.add(message.code);
      else this.keys.delete(message.code);
    } else if (message.type === 'mouse-button') {
      if (message.down) this.buttons.add(message.button);
      else this.buttons.delete(message.button);
    }
  }

  /** The releases for everything still down; forgets it. */
  releaseAll(): InputMessage[] {
    const out: InputMessage[] = [
      ...[...this.keys].map((code) => ({ v: 1, type: 'key', code, down: false }) as const),
      ...[...this.buttons].map((button) => ({ v: 1, type: 'mouse-button', button, down: false }) as const),
    ];
    this.keys.clear();
    this.buttons.clear();
    return out;
  }
}
