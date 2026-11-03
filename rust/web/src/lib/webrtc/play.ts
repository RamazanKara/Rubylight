// The play page's client (docs/webrtc.md, "Play in a TV browser"): protocol v2 on the guest
// port's `/play/signal`, as a TV speaks it on `webrtc_port`. The page pairs once with a PIN the
// console confirms, keeps the device's ID and secret in localStorage, and on every visit signs in
// with a proof of the secret. The stream is the console Stream page's, with this browser's pads
// on the host's `control` and `gamepad` channels. Navigation is pure here, so Play.test.mjs
// tests it in Node.
import { ConsoleStream, type Capabilities } from './client';
import { Pads, rumble, type PadSnapshot } from './gamepad';
import { Signalling, SignallingError } from './signalling';

export const PLAY_PROTOCOL = 'rubylight.play.v2';
export const CREDENTIALS_KEY = 'rubylight.play.device';
const PROOF_LABEL = 'moonlight-webrtc-tv-auth-v1:';
/** A pairing request lasts 5 minutes on the host; the page waits a little longer for its answer. */
const PAIRING_TIMEOUT_MS = 5 * 60_000 + 10_000;
/** How long Back or Select is held to open the stream menu. */
export const HOLD_MS = 1000;

export type Credentials = { clientId: string; secret: string };

export function playSignalUrl(page: { protocol: string; host: string }): string {
  return `${page.protocol === 'https:' ? 'wss' : 'ws'}://${page.host}/play/signal`;
}

/** A four-digit PIN from the browser's random source. */
export function newPin(random: (array: Uint32Array) => unknown = (array) => crypto.getRandomValues(array)): string {
  const value = new Uint32Array(1);
  random(value);
  return String(value[0]! % 10_000).padStart(4, '0');
}

const hex = (value: unknown, bytes: number): value is string => typeof value === 'string' && new RegExp(`^[0-9a-f]{${bytes * 2}}$`).test(value);

/** Saved credentials when they are well formed: a 128-bit ID and a 256-bit secret in hex. */
export function parseCredentials(text: string | null): Credentials | null {
  try {
    const value = JSON.parse(text ?? '') as Record<string, unknown> | null;
    if (hex(value?.clientId, 16) && hex(value?.secret, 32)) return { clientId: value.clientId, secret: value.secret };
  } catch {
    // Not saved by this page.
  }
  return null;
}

/** The browser's saved credentials; null when there are none or site data is blocked. */
export function loadCredentials(storage: Pick<Storage, 'getItem'> = localStorage): Credentials | null {
  try {
    return parseCredentials(storage.getItem(CREDENTIALS_KEY));
  } catch {
    return null;
  }
}

/** Throws when the browser keeps no site data: it would have to pair on every visit. */
export function saveCredentials(credentials: Credentials, storage: Pick<Storage, 'setItem'> = localStorage) {
  try {
    storage.setItem(CREDENTIALS_KEY, JSON.stringify(credentials));
  } catch (error) {
    throw new SignallingError(`This browser does not keep site data, so it has to pair again next time (${String(error)}).`);
  }
}

export function forgetCredentials(storage: Pick<Storage, 'removeItem'> = localStorage) {
  try {
    storage.removeItem(CREDENTIALS_KEY);
  } catch {
    // Nothing was kept.
  }
}

/** `HMAC-SHA256(secret, label + nonce)` in lower-case hex, the proof `authenticate` carries. */
export async function proof(secret: string, nonce: string, subtle: SubtleCrypto | undefined = globalThis.crypto?.subtle): Promise<string> {
  if (!subtle) throw new SignallingError('This page must be opened over https:// to sign in.');
  const bytes = new Uint8Array(secret.match(/../g)!.map((pair) => parseInt(pair, 16)));
  const key = await subtle.importKey('raw', bytes, { name: 'HMAC', hash: 'SHA-256' }, false, ['sign']);
  const signature = await subtle.sign('HMAC', key, new TextEncoder().encode(PROOF_LABEL + nonce));
  return [...new Uint8Array(signature)].map((b) => b.toString(16).padStart(2, '0')).join('');
}

/** The name the PC's Devices page shows for this browser. */
export function deviceName(agent: string): string {
  if (/Xbox/i.test(agent)) return 'Xbox';
  if (/Web0S|webOS/i.test(agent)) return 'LG TV';
  if (/Tizen/i.test(agent)) return 'Samsung TV';
  if (/Tesla|QtCarBrowser/i.test(agent)) return 'Car';
  if (/Android/i.test(agent) && /\bTV\b|AFT|BRAVIA|SmartTV/i.test(agent)) return 'Android TV';
  const browser = /Edg\//.test(agent) ? 'Edge' : /Firefox\//.test(agent) ? 'Firefox' : /Chrome\//.test(agent) ? 'Chrome' : /Safari\//.test(agent) ? 'Safari' : 'Browser';
  const system = /Windows/.test(agent) ? 'Windows' : /Android/.test(agent) ? 'Android' : /CrOS/.test(agent) ? 'ChromeOS' : /Mac OS X/.test(agent) ? 'Mac' : /Linux/.test(agent) ? 'Linux' : '';
  return system ? `${browser} on ${system}` : browser;
}

/** The socket and the nonce of its `auth-required`. */
async function opened(url: string): Promise<{ sig: Signalling; nonce: string }> {
  const sig = await Signalling.openWith(url, [PLAY_PROTOCOL]);
  try {
    const required = await sig.expect('auth-required');
    return { sig, nonce: String(required.nonce ?? '') };
  } catch (error) {
    sig.close();
    throw error;
  }
}

/** The play page's socket and the stream it runs, signed in as this browser's device. */
export class PlayStream extends ConsoleStream {
  gatewayName = '';
  private control: RTCDataChannel | null = null;
  private pad: RTCDataChannel | null = null;
  private pads = new Pads();

  /** Signs in with saved credentials; rejects with the host's code, such as `authentication-failed`. */
  static async signIn(credentials: Credentials, url = playSignalUrl(location)): Promise<PlayStream> {
    const { sig, nonce } = await opened(url);
    try {
      sig.send('authenticate', { clientId: credentials.clientId, proof: await proof(credentials.secret, nonce) });
      await sig.expect('authenticated', 15_000, 'authenticate');
      return await PlayStream.ready(sig);
    } catch (error) {
      sig.close();
      throw error;
    }
  }

  /**
   * Asks to pair with `pin`, which the PC's console must confirm. `result` resolves with the
   * signed-in stream and the new credentials, or rejects with the host's code (`incorrect-pin`,
   * `pairing-expired`, ...). `cancel` closes the socket, which withdraws the request.
   */
  static async pairing(pin: string, name: string, url = playSignalUrl(location)): Promise<{ result: Promise<{ stream: PlayStream; credentials: Credentials }>; cancel: () => void }> {
    const { sig } = await opened(url);
    const result = sig.request('request-pairing', { pin, clientName: name }, 'paired', PAIRING_TIMEOUT_MS).then(
      async (paired) => ({ stream: await PlayStream.ready(sig), credentials: { clientId: String(paired.clientId), secret: String(paired.clientSecret) } }),
      (error: unknown) => {
        sig.close();
        throw error;
      },
    );
    return { result, cancel: () => sig.close() };
  }

  private static async ready(sig: Signalling): Promise<PlayStream> {
    const stream = new PlayStream(sig);
    stream.gatewayName = String((await sig.expect('gateway-status')).gatewayName ?? '');
    stream.capabilities = (await sig.expect('capabilities')) as Capabilities;
    return stream;
  }

  /** Why the host closed the socket, such as "Opened in another tab or browser". */
  get closeReason(): string {
    return this.sig.closeReason;
  }

  /** The app's cover as a `data:` URL; null when it has none. One request at a time. */
  async artwork(appId: string): Promise<string | null> {
    const reply = await this.sig.request('get-app-artwork', { appId }, 'app-artwork');
    if (!reply.available || reply.appId !== appId || typeof reply.data !== 'string') return null;
    return `data:${String(reply.mimeType)};base64,${reply.data}`;
  }

  protected override opened(pc: RTCPeerConnection) {
    this.control = this.pad = null;
    this.pads = new Pads();
    pc.ondatachannel = (event) => {
      if (event.channel.label === 'control') {
        this.control = event.channel;
        event.channel.onmessage = (message) => rumble(String(message.data));
      } else if (event.channel.label === 'gamepad') this.pad = event.channel;
    };
  }

  /** Sends what changed on this browser's pads; `idle` sends them as let go, for the menu. */
  pollPads(pads: readonly (PadSnapshot | null)[], now: number, idle = false) {
    if (this.control?.readyState !== 'open' || this.pad?.readyState !== 'open') return;
    const { control, gamepad } = this.pads.poll(idle ? pads.map((pad) => pad && released(pad)) : pads, now);
    for (const message of control) this.control.send(JSON.stringify(message));
    for (const message of gamepad) this.pad.send(JSON.stringify(message));
  }
}

/** A pad with nothing pressed and its sticks centred. */
export function released(pad: PadSnapshot): PadSnapshot {
  return { ...pad, buttons: pad.buttons.map(() => ({ pressed: false, value: 0 })), axes: pad.axes.map(() => 0) };
}

export type Move = 'up' | 'down' | 'left' | 'right';
export type Action = Move | 'select' | 'back' | 'menu';
type Box = { left: number; top: number; width: number; height: number };

/**
 * The box nearest to `from` in `move`'s direction, by distance along it with sideways distance
 * weighed double: a grid moves along its rows and columns. -1 when there is none that way.
 */
export function nearest(from: Box, boxes: readonly Box[], move: Move): number {
  const centre = (b: Box) => [b.left + b.width / 2, b.top + b.height / 2] as const;
  const [x, y] = centre(from);
  let best = -1;
  let bestScore = Infinity;
  boxes.forEach((box, index) => {
    const [bx, by] = centre(box);
    const along = { up: y - by, down: by - y, left: x - bx, right: bx - x }[move];
    const across = move === 'up' || move === 'down' ? Math.abs(bx - x) : Math.abs(by - y);
    if (along <= 1) return;
    const score = along + 2 * across;
    if (score < bestScore) [best, bestScore] = [index, score];
  });
  return best;
}

/** A key's action on the page: arrows and a TV remote's D-pad, OK and Back. */
export function keyAction(event: Pick<KeyboardEvent, 'key' | 'keyCode'>): Action | null {
  switch (event.key) {
    case 'ArrowUp':
      return 'up';
    case 'ArrowDown':
      return 'down';
    case 'ArrowLeft':
      return 'left';
    case 'ArrowRight':
      return 'right';
    case 'Escape':
    case 'GoBack':
    case 'BrowserBack':
    case 'XF86Back':
      return 'back';
  }
  // webOS and Tizen remotes' Back.
  return event.keyCode === 461 || event.keyCode === 10009 ? 'back' : null;
}

/** The pad buttons the page reads: A, B, Back (View, Select) and the D-pad. */
const PAD = { select: 0, back: 1, menu: 8, up: 12, down: 13, left: 14, right: 15 } as const;
const REPEAT_FIRST_MS = 400;
const REPEAT_MS = 150;
const STICK = 0.6;

/**
 * Pad presses as page actions: a press acts once, a held direction repeats, and the left stick
 * moves like the D-pad. `menu` comes once Back or Select is held for HOLD_MS.
 */
export class PadNavigator {
  private down = new Map<string, { since: number; next: number; fired: boolean }>();

  poll(pads: readonly (PadSnapshot | null)[], now: number): Action[] {
    const held = new Set<Action>();
    for (const pad of pads) {
      if (!pad) continue;
      for (const [action, index] of Object.entries(PAD) as [Action, number][]) if (pad.buttons[index]?.pressed) held.add(action);
      const [sx = 0, sy = 0] = pad.axes;
      if (sy < -STICK) held.add('up');
      if (sy > STICK) held.add('down');
      if (sx < -STICK) held.add('left');
      if (sx > STICK) held.add('right');
    }
    const actions: Action[] = [];
    for (const action of [...this.down.keys()]) if (!held.has(action as Action)) this.down.delete(action);
    for (const action of held) {
      const state = this.down.get(action);
      if (!state) {
        this.down.set(action, { since: now, next: now + REPEAT_FIRST_MS, fired: false });
        if (action !== 'menu') actions.push(action);
      } else if (action === 'menu') {
        if (!state.fired && now - state.since >= HOLD_MS) {
          state.fired = true;
          actions.push('menu');
        }
      } else if (action !== 'select' && action !== 'back' && now >= state.next) {
        state.next = now + REPEAT_MS;
        actions.push(action);
      }
    }
    return actions;
  }
}

/** One button or key held for HOLD_MS: true once per hold. */
export class Hold {
  private since: number | null = null;
  private fired = false;

  update(pressed: boolean, now: number): boolean {
    if (!pressed) {
      this.since = null;
      this.fired = false;
      return false;
    }
    this.since ??= now;
    if (this.fired || now - this.since < HOLD_MS) return false;
    this.fired = true;
    return true;
  }
}
