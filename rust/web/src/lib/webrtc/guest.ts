// The guest page's client (docs/browser-guests.md): its socket at `/guest/signal` sends the
// invite token first, takes the host's offer for the owner's stream and answers it. Picture and
// sound come as in the console's Stream page; what the guest may send follows the invite: a
// view-only guest sends nothing, a controller guest its pads, and with keyboard and mouse also the
// `input` channel. The host checks all of it again.
import { Pads, rumble, type PadMessage, type PadSnapshot } from './gamepad';
import { browserOffer, stereoAnswer } from './client';
import { Held, INPUT_LABEL, INPUT_OPTIONS, type InputMessage } from './input';

export const GUEST_PROTOCOL = 'rubylight.guest.v1';

export type Permission = 'view' | 'controller' | 'controller-keyboard-mouse';

/** What the host's offer says about the stream the guest joins. */
export type Joined = { name: string; permission: Permission; codec: string; width: number; height: number; fps: number };

/**
 * How a guest's stream ended or why it did not start. `wait`: the link still works and the page
 * joins again by itself (the owner is not streaming yet, or the owner's stream ended); `final`:
 * the link opens nothing any more.
 */
export type Ending = { code: string; message: string; wait: boolean; final: boolean };

export function guestSignalUrl(page: { protocol: string; host: string }): string {
  return `${page.protocol === 'https:' ? 'wss' : 'ws'}://${page.host}/guest/signal`;
}

/** The token in the link's fragment, which never reaches a server log; null when it is not one. */
export function inviteToken(hash: string): string | null {
  const match = /^#?([0-9a-f]{32})$/.exec(hash.trim().toLowerCase());
  return match ? match[1]! : null;
}

export const gamepads = (permission: Permission) => permission !== 'view';
export const keyboardMouse = (permission: Permission) => permission === 'controller-keyboard-mouse';

const LOST = 'The connection to the host was lost.';
const REFUSED = 'The host refused this page’s connection.';
/** What the host says of a link that opens nothing; its guest port answers 404 once no link works. */
const INVALID_LINK = 'This invite link does not work: it expired, the host withdrew it, or it was copied incompletely.';

/** The status the guest page's own address answers with now. */
type Probe = () => Promise<number>;
const pageStatus: Probe = () => fetch(location.pathname, { method: 'HEAD', cache: 'no-store' }).then((response) => response.status);

/** The host's `error` or `ended` message as the page shows it. */
export function ending(message: { type?: unknown; code?: unknown; reason?: unknown; message?: unknown }): Ending {
  const code = String((message.type === 'ended' ? message.reason : message.code) ?? 'connection-lost');
  const text = typeof message.message === 'string' && message.message ? message.message : LOST;
  return { code, message: text, wait: code === 'not-streaming' || code === 'ended', final: code === 'invalid-link' || code === 'revoked' };
}

type Socket = Pick<WebSocket, 'send' | 'close' | 'protocol'> & {
  onopen: ((event: Event) => void) | null;
  onclose: ((event: CloseEvent) => void) | null;
  onerror: ((event: Event) => void) | null;
  onmessage: ((event: MessageEvent) => void) | null;
};
type SocketFactory = (url: string, protocols: string[]) => Socket;

/** One guest's socket and stream. */
export class GuestStream {
  joined: Joined | null = null;
  /** Called once, when the stream ended or never started. */
  onended: ((ending: Ending) => void) | null = null;
  private pc: RTCPeerConnection | null = null;
  private input: RTCDataChannel | null = null;
  private control: RTCDataChannel | null = null;
  private pad: RTCDataChannel | null = null;
  private held = new Held();
  private pads = new Pads();
  private over = false;

  private constructor(private ws: Socket) {}

  /**
   * Opens the socket and joins with `token`; plays the stream in `media` once the host offers
   * it. `onstreaming` runs when the picture connected. A socket that never opened asks `probe`
   * why, since a browser does not say: a 404 means no link works on the host any more.
   */
  static join(
    token: string,
    name: string,
    media: MediaStream,
    onstreaming: (stream: GuestStream) => void,
    url = guestSignalUrl(location),
    open: SocketFactory = (u, p) => new WebSocket(u, p),
    probe: Probe = pageStatus,
  ): GuestStream {
    const stream = new GuestStream(open(url, [GUEST_PROTOCOL]));
    const ws = stream.ws;
    let opened = false;
    ws.onopen = () => {
      opened = true;
      if (ws.protocol !== GUEST_PROTOCOL) {
        stream.end({ code: 'protocol', message: `The host answered with the subprotocol "${ws.protocol}", not ${GUEST_PROTOCOL}.`, wait: false, final: false });
        return;
      }
      ws.send(JSON.stringify({ type: 'join', token, name }));
    };
    let refused = false;
    const notOpened = () => {
      if (refused) return;
      refused = true;
      const end = (status?: number) =>
        stream.end(
          status === 404
            ? { code: 'invalid-link', message: INVALID_LINK, wait: false, final: true }
            : { code: 'refused', message: REFUSED, wait: false, final: false },
        );
      probe().then(end, () => end());
    };
    ws.onerror = () => {
      if (!opened) notOpened();
    };
    ws.onclose = () => (opened ? stream.end(ending({})) : notOpened());
    ws.onmessage = (event) => {
      let message: Record<string, unknown>;
      try {
        message = JSON.parse(String(event.data)) as Record<string, unknown>;
      } catch {
        return;
      }
      if (message.type === 'offer') {
        stream.answer(message, media).catch((error: unknown) => {
          stream.end({ code: 'browser', message: `This browser could not play the stream: ${error instanceof Error ? error.message : String(error)}`, wait: false, final: false });
        });
      } else if (message.type === 'streaming') {
        void stream.streaming().then(
          () => !stream.over && onstreaming(stream),
          () => stream.end({ code: 'input', message: 'The host closed the input channel.', wait: false, final: false }),
        );
      } else if (message.type === 'error' || message.type === 'ended') {
        stream.end(ending(message));
      }
    };
    return stream;
  }

  private async answer(offer: Record<string, unknown>, media: MediaStream) {
    const permission = String(offer.permission) as Permission;
    this.joined = {
      name: String(offer.name ?? ''),
      permission,
      codec: String(offer.codec ?? ''),
      width: Number(offer.width) || 0,
      height: Number(offer.height) || 0,
      fps: Number(offer.fps) || 0,
    };
    const pc = (this.pc = new RTCPeerConnection({ iceServers: [] }));
    pc.ontrack = (event) => {
      const receiver = event.receiver as RTCRtpReceiver & { jitterBufferTarget?: number | null };
      if ('jitterBufferTarget' in receiver) receiver.jitterBufferTarget = 0;
      media.addTrack(event.track);
    };
    pc.ondatachannel = (event) => {
      if (event.channel.label === 'control') {
        this.control = event.channel;
        event.channel.onmessage = (message) => rumble(String(message.data));
      } else if (event.channel.label === 'gamepad') this.pad = event.channel;
    };
    pc.onconnectionstatechange = () => {
      if (pc.connectionState === 'failed') this.end({ code: 'connection-lost', message: 'The stream’s connection to the host failed.', wait: false, final: false });
    };
    await pc.setRemoteDescription({ type: 'offer', sdp: browserOffer(String(offer.sdp)) });
    const answer = await pc.createAnswer();
    await pc.setLocalDescription({ type: 'answer', sdp: stereoAnswer(answer.sdp ?? '') });
    if (this.over) return;
    this.ws.send(JSON.stringify({ type: 'answer', sdp: pc.localDescription?.sdp }));
  }

  /** The picture connected: open the `input` channel when the invite allows keyboard and mouse. */
  private async streaming() {
    if (!this.pc || !this.joined || !keyboardMouse(this.joined.permission)) return;
    const input = this.pc.createDataChannel(INPUT_LABEL, INPUT_OPTIONS);
    await new Promise<void>((resolve, reject) => {
      input.onopen = () => resolve();
      input.onclose = () => reject(new Error('closed'));
    });
    input.onclose = () => this.end({ code: 'input', message: 'The host closed the input channel.', wait: false, final: false });
    this.input = input;
  }

  /** Keyboard and mouse, when the invite allows them; dropped otherwise. */
  send(message: InputMessage | null) {
    if (!message || this.input?.readyState !== 'open') return;
    this.held.track(message);
    this.input.send(JSON.stringify(message));
  }

  releaseAll() {
    for (const message of this.held.releaseAll()) this.send(message);
  }

  /** Sends what changed on this browser's pads, when the invite allows them. */
  pollPads(pads: readonly (PadSnapshot | null)[], now: number) {
    if (!this.joined || !gamepads(this.joined.permission) || this.control?.readyState !== 'open' || this.pad?.readyState !== 'open') return;
    const { control, gamepad } = this.pads.poll(pads, now);
    for (const message of control) this.control.send(JSON.stringify(message));
    for (const message of gamepad) this.pad.send(JSON.stringify(message));
  }

  /** Leaves the owner's stream; the page says nothing more. */
  leave() {
    if (this.over) return;
    this.releaseAll();
    this.unplug();
    try {
      this.ws.send(JSON.stringify({ type: 'leave' }));
    } catch {
      // The socket is gone already.
    }
    this.onended = null;
    this.end(ending({}));
  }

  private unplug() {
    if (this.control?.readyState !== 'open') return;
    for (const message of this.pads.release() as PadMessage[]) this.control.send(JSON.stringify(message));
  }

  private end(why: Ending) {
    if (this.over) return;
    this.over = true;
    this.ws.onclose = null;
    this.ws.close();
    if (this.input) this.input.onclose = null;
    this.input?.close();
    this.pc?.close();
    this.pc = this.input = this.control = this.pad = null;
    this.onended?.(why);
  }
}
