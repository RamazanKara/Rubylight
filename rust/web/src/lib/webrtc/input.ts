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

type Size = { width: number; height: number };

/**
 * The part of the video that is the picture. AMD encodes AV1 at sizes like 1920×1080 padded (as
 * 1920×1082, AMF issue 423) and browsers don't crop it, so what lies past the stream's own size
 * by no more than that padding is left out. Any other difference is a real size and stays.
 */
export function shownSize(video: Size, wanted: Size): Size {
  const crop = (size: number, want: number, padding: number) => (want > 0 && size > want && size - want < padding ? want : size);
  return { width: crop(video.width, wanted.width, 64), height: crop(video.height, wanted.height, 16) };
}

/** The CSS clip that hides the padding on the right and below, for a video letterboxed in `box`. */
export function pictureClip(box: Size, video: Size, shown: Size): string {
  if (!video.width || !video.height || (shown.width === video.width && shown.height === video.height)) return '';
  const scale = Math.min(box.width / video.width, box.height / video.height);
  const right = (box.width - video.width * scale) / 2 + (video.width - shown.width) * scale;
  const bottom = (box.height - video.height * scale) / 2 + (video.height - shown.height) * scale;
  return `inset(0 ${right}px ${bottom}px 0)`;
}

/** Keeps the element's clip right as the picture and the element change size; returns the stop. */
export function cropPadding(video: HTMLVideoElement, wanted: Size): () => void {
  const apply = () => {
    const size = { width: video.videoWidth, height: video.videoHeight };
    video.style.clipPath = pictureClip({ width: video.clientWidth, height: video.clientHeight }, size, shownSize(size, wanted));
  };
  apply();
  video.addEventListener('resize', apply);
  const observer = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(apply);
  observer?.observe(video);
  return () => {
    video.removeEventListener('resize', apply);
    observer?.disconnect();
    video.style.clipPath = '';
  };
}

/**
 * Where a pointer at (clientX, clientY) is on the picture, as a fraction of it: the element
 * letterboxes the video (object-fit: contain), so the bars are left out, and `shown` leaves out
 * the padding below and to the right. Null before the first frame, when the size is unknown.
 */
export function positionMessage(
  clientX: number,
  clientY: number,
  box: { left: number; top: number; width: number; height: number },
  video: Size,
  shown: Size = video,
): InputMessage | null {
  if (!video.width || !video.height || !box.width || !box.height) return null;
  const scale = Math.min(box.width / video.width, box.height / video.height);
  const left = box.left + (box.width - video.width * scale) / 2;
  const top = box.top + (box.height - video.height * scale) / 2;
  return { v: 1, type: 'mouse-position', x: clamp((clientX - left) / (shown.width * scale)), y: clamp((clientY - top) / (shown.height * scale)) };
}

/** The bit of each DOM button (0 to 4) in `MouseEvent.buttons`: middle and right swap places. */
const BUTTON_BITS = [1, 4, 2, 8, 16];

/**
 * The presses and releases between two `MouseEvent.buttons` masks. A browser reports a second
 * button pressed or released while another is held as a `pointermove` only, so the page follows
 * the mask on every pointer event instead of `pointerdown` and `pointerup`.
 */
export function buttonChanges(previous: number, current: number): InputMessage[] {
  const out: InputMessage[] = [];
  BUTTON_BITS.forEach((bit, button) => {
    if ((previous & bit) !== (current & bit)) out.push({ v: 1, type: 'mouse-button', button, down: (current & bit) !== 0 });
  });
  return out;
}

/** WheelEvent.deltaMode: pixels, lines, pages. */
const PIXEL = 0;
const LINE = 1;
/** One notch is 120 units; Chromium scrolls 100 pixels and Firefox 3 lines per notch. */
const UNITS_PER_PIXEL = 1.2;
const UNITS_PER_LINE = 40;
const UNITS_PER_PAGE = 360;

/**
 * Wheel events in 120 units per notch with DOM signs (positive `dy` scrolls down). What does not
 * make a whole unit is carried to the next event, so a touchpad's small deltas add up.
 */
export class Wheel {
  private x = 0;
  private y = 0;

  message(deltaX: number, deltaY: number, deltaMode: number): InputMessage | null {
    const unit = deltaMode === PIXEL ? UNITS_PER_PIXEL : deltaMode === LINE ? UNITS_PER_LINE : UNITS_PER_PAGE;
    this.x += deltaX * unit;
    this.y += deltaY * unit;
    // The epsilon keeps 119.99999 from rounding error a whole notch.
    const dx = Math.trunc(this.x + Math.sign(this.x) * 1e-9);
    const dy = Math.trunc(this.y + Math.sign(this.y) * 1e-9);
    this.x -= dx;
    this.y -= dy;
    return dx || dy ? { v: 1, type: 'wheel', dx, dy } : null;
  }
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
