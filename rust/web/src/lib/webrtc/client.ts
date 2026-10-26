// The Stream page's protocol v2 client: H.264 1080p60 and stereo Opus from the host's offer,
// keyboard and mouse on the `input` channel. Ported from rust/tests/webrtc_client/client.js;
// codecs, surround, gamepads and settings come later (docs/webrtc.md, "Stream in the console").
import { Held, INPUT_LABEL, INPUT_OPTIONS, type InputMessage } from './input';
import { Signalling, SignallingError, signalUrl, type Message } from './signalling';

/** What this page asks for. Every browser decodes H.264. */
export const STREAM = { width: 1920, height: 1080, fps: 60, codec: 'h264', channels: 2 } as const;

export type AppEntry = { id: string; title: string; artworkAvailable: boolean; running: boolean };

type VideoMode = { width: number; height: number; frameRates: number[]; codecs: string[]; defaultBitrateKbps: number };
type CodecFrameRates = { width: number; height: number; codec: string; frameRates: number[] };
export type Capabilities = {
  videoModes?: VideoMode[];
  extensions?: {
    input?: { label: string; version: number; keyboard: boolean; mouse: boolean };
    codecFrameRates?: CodecFrameRates[];
  };
};

/**
 * The bitrate for STREAM when the host offers it, from the mode's default. Throws when the host
 * cannot send H.264 at 1080p60 or does not take keyboard and mouse: there is no fallback.
 */
export function streamBitrate(capabilities: Capabilities): number {
  const { width, height, fps, codec } = STREAM;
  const mode = capabilities.videoModes?.find((m) => m.width === width && m.height === height);
  const rates = capabilities.extensions?.codecFrameRates?.find((c) => c.width === width && c.height === height && c.codec === codec)?.frameRates;
  const offered = mode && (rates ?? (mode.codecs.includes(codec) ? mode.frameRates : [])).includes(fps);
  if (!mode || !offered) throw new SignallingError(`The host does not offer H.264 at ${width}×${height} and ${fps} fps.`);
  const input = capabilities.extensions?.input;
  if (input?.label !== INPUT_LABEL || !input.keyboard || !input.mouse) {
    throw new SignallingError('The host does not take keyboard and mouse from the console.');
  }
  return mode.defaultBitrateKbps;
}

/**
 * The answer with `stereo=1` on its Opus format: Chromium decodes from its own description, and
 * plays a stereo stream as one channel without it. Throws when the answer has no stereo Opus.
 */
export function stereoAnswer(sdp: string): string {
  const opus = /^a=rtpmap:(\d+) opus\/48000\/2\r?$/im.exec(sdp);
  if (!opus) throw new SignallingError('The browser answered without stereo Opus audio.');
  const type = opus[1];
  const fmtp = new RegExp(`^a=fmtp:${type} [^\\r\\n]*`, 'm');
  if (!fmtp.test(sdp)) return sdp.replace(/^a=rtpmap:\d+ opus\/48000\/2(?=\r?$)/im, (line) => `${line}\r\na=fmtp:${type} stereo=1;sprop-stereo=1`);
  return sdp.replace(fmtp, (line) => (/(^|[ ;])stereo=1/.test(line) ? line : `${line};stereo=1;sprop-stereo=1`));
}

/** Whether this browser can receive H.264 over WebRTC. */
export function receivesH264(capabilities: RTCRtpCapabilities | null | undefined): boolean {
  return !!capabilities?.codecs.some((c) => c.mimeType.toLowerCase() === 'video/h264');
}

type Receiver = RTCRtpReceiver & { jitterBufferTarget?: number | null; playoutDelayHint?: number | null };

/** No playout delay: the host sends each frame the moment it exists. */
function noPlayoutDelay(receiver: Receiver) {
  if ('jitterBufferTarget' in receiver) receiver.jitterBufferTarget = 0;
  if ('playoutDelayHint' in receiver) receiver.playoutDelayHint = 0;
}

const CONNECT_TIMEOUT_MS = 60_000;
const timeout = (ms: number, what: string) =>
  new Promise<never>((_, reject) => setTimeout(() => reject(new SignallingError(`${what} did not happen within ${ms / 1000} s.`)), ms));

/** The console's stream socket, and the stream it runs. */
export class ConsoleStream {
  capabilities: Capabilities = {};
  private pc: RTCPeerConnection | null = null;
  private input: RTCDataChannel | null = null;
  private held = new Held();
  private stopListening: (() => void) | null = null;
  /** Rejects the start in progress. */
  private abort: ((error: Error) => void) | null = null;
  /** The stream ended on its own: the host stopped it, the connection failed, or the socket closed. */
  onended: ((reason: string) => void) | null = null;

  private constructor(private sig: Signalling) {
    sig.onclose = () => this.end(sig.closeReason ? `The host closed the stream socket: ${sig.closeReason}.` : 'The host closed the stream socket.');
  }

  /** Opens the console's socket, which the host has signed in already. */
  static async connect(csrf: string, url = signalUrl(location)): Promise<ConsoleStream> {
    const sig = await Signalling.open(url, csrf);
    try {
      await sig.expect('authenticated');
      const stream = new ConsoleStream(sig);
      stream.capabilities = (await sig.expect('capabilities')) as Capabilities;
      return stream;
    } catch (error) {
      sig.close();
      throw error;
    }
  }

  async apps(): Promise<AppEntry[]> {
    this.sig.clear();
    this.sig.send('get-apps');
    return ((await this.sig.expect('apps')).apps ?? []) as AppEntry[];
  }

  /**
   * Starts `appId` and plays it in `video`; resolves once the input channel is open. A failure
   * stops the half-started session on the host before it rejects.
   */
  async start(appId: string, video: HTMLVideoElement, status: (text: string) => void): Promise<void> {
    if (!receivesH264(RTCRtpReceiver.getCapabilities?.('video'))) {
      throw new SignallingError('This browser cannot receive H.264 over WebRTC.');
    }
    const bitrateKbps = streamBitrate(this.capabilities);
    let fail: (error: Error) => void = () => {};
    const failed = new Promise<never>((_, reject) => (fail = reject));
    failed.catch(() => {});
    this.abort = fail;
    try {
      await this.connectStream(appId, bitrateKbps, video, status, failed, fail);
    } catch (error) {
      // stop() already told the host when the page gave up.
      if (this.abort) {
        this.abort = null;
        this.teardown();
        if (!this.sig.closed) this.sig.send('stop-session');
      }
      throw error;
    }
    this.abort = null;
  }

  private async connectStream(
    appId: string,
    bitrateKbps: number,
    video: HTMLVideoElement,
    status: (text: string) => void,
    failed: Promise<never>,
    fail: (error: Error) => void,
  ) {
    this.stopListening = this.sig.on((message: Message) => {
      if (message.type === 'error') {
        const error = new SignallingError(String(message.message ?? message.code), String(message.code ?? ''));
        fail(error);
        if (this.input) this.end(error.message);
      } else if (message.type === 'session-status') {
        const text = message.message ? `: ${String(message.message)}` : '';
        if (message.state === 'error') {
          fail(new SignallingError(`The host could not stream${text}.`));
          this.end(`The host ended the stream${text}.`);
        } else if (message.state === 'idle' && this.input) this.end(`The stream ended${text}.`);
        else status(`Host: ${String(message.state)}${text}`);
      }
    });
    const { width, height, fps, codec, channels } = STREAM;
    status('Starting the app…');
    this.sig.clear();
    this.sig.send('start-session', { appId, video: { width, height, fps, codec, bitrateKbps, hdr: false }, audio: { channels } });
    const offer = await Promise.race([this.sig.expect('offer', CONNECT_TIMEOUT_MS), failed]);
    const sessionId = offer.sessionId;
    const pc = (this.pc = new RTCPeerConnection({ iceServers: [] }));
    const media = new MediaStream();
    pc.ontrack = (event) => {
      noPlayoutDelay(event.receiver);
      media.addTrack(event.track);
      if (video.srcObject !== media) {
        video.srcObject = media;
        video.play().catch((error: unknown) => {
          fail(new SignallingError(`The browser did not play the stream: ${String(error)}`));
          if (this.input) this.end(`The browser did not play the stream: ${String(error)}`);
        });
      }
    };
    const connected = new Promise<void>((resolve, reject) => {
      pc.onconnectionstatechange = () => {
        if (pc.connectionState === 'connected') resolve();
        else if (pc.connectionState === 'failed') {
          reject(new SignallingError('The peer connection failed. Streaming works on the local network and Tailscale only.'));
          this.end('The connection to the host failed.');
        }
      };
    });
    status('Connecting…');
    await pc.setRemoteDescription({ type: 'offer', sdp: String(offer.sdp) });
    const answer = await pc.createAnswer();
    await pc.setLocalDescription({ type: 'answer', sdp: stereoAnswer(answer.sdp ?? '') });
    this.sig.send('answer', { sessionId, sdp: pc.localDescription?.sdp });
    await Promise.race([connected, failed, timeout(CONNECT_TIMEOUT_MS, 'The peer connection')]);
    const input = pc.createDataChannel(INPUT_LABEL, INPUT_OPTIONS);
    await Promise.race([
      new Promise<void>((resolve, reject) => {
        input.onopen = () => resolve();
        input.onclose = () => reject(new SignallingError('The host closed the input channel.'));
      }),
      failed,
      timeout(10_000, 'Opening the input channel'),
    ]);
    input.onclose = () => this.end('The host closed the input channel.');
    this.input = input;
    status('');
  }

  /** Sends one input message; dropped when no stream runs. */
  send(message: InputMessage | null) {
    if (!message || this.input?.readyState !== 'open') return;
    this.held.track(message);
    this.input.send(JSON.stringify(message));
  }

  /** Lets go of every key and button the host holds for this page. */
  releaseAll() {
    for (const message of this.held.releaseAll()) this.send(message);
  }

  /** Ends the stream; `quitApp` also quits the app on the host. */
  stop(quitApp: boolean) {
    const starting = this.abort;
    if (!this.pc && !starting) return;
    this.abort = null;
    this.releaseAll();
    if (!this.sig.closed) {
      this.sig.send('stop-session');
      if (quitApp) this.sig.send('stop-host-session');
    }
    this.teardown();
    starting?.(new SignallingError('The stream was stopped.'));
  }

  /** Stops the stream and closes the socket, when the page goes away. */
  close() {
    this.stop(false);
    this.sig.onclose = null;
    this.sig.close();
  }

  private end(reason: string) {
    if (!this.pc && !this.sig.closed) return;
    this.teardown();
    this.onended?.(reason);
  }

  private teardown() {
    this.stopListening?.();
    this.stopListening = null;
    this.input?.close();
    this.input = null;
    this.pc?.close();
    this.pc = null;
  }
}
