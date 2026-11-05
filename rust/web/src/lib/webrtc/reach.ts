// Why a browser's connection to the PC failed, in words. A browser says only "failed", and takes
// 5 to 15 s to say it; its candidate pairs tell a browser that never tried from a PC that never
// answered, and those have different fixes (docs/troubleshooting.md).

/** A check takes one round trip and browsers repeat them many times a second: by now none will come back. */
export const NO_ANSWER_MS = 8_000;
/** From the first answer to connected. A first frame has taken 5.5 s under loss, handshake included, and this leaves it several times that. */
export const NO_HANDSHAKE_MS = 30_000;
const LOOK_EVERY_MS = 1_000;

/**
 * - `no-try`: the browser made no candidate pair, so it sent nothing.
 * - `no-answer`: it sent checks and nothing came back.
 * - `no-handshake`: the PC answered, but the encrypted connection failed.
 * - `lost`: the stream had connected, and the connection dropped.
 */
export type Failure = 'no-try' | 'no-answer' | 'no-handshake' | 'lost';

/** Where the host's offer asks the browser to send its media. */
export type Offered = { address: string; port: number };

export function offeredAddress(sdp: string): Offered | null {
  const match = /^a=candidate:\S+ \d+ udp \d+ (\S+) (\d+) typ host/im.exec(sdp);
  return match ? { address: match[1]!, port: Number(match[2]) } : null;
}

const isLoopback = (address: string) => address === '::1' || address.startsWith('127.');

/** What the Stream and play pages say, to whoever runs the PC. */
export function consoleFailure(why: Failure, offered: Offered | null): string {
  const where = offered ? `${offered.address}, UDP port ${offered.port}` : 'its media port';
  switch (why) {
    case 'no-try':
      if (offered && isLoopback(offered.address)) {
        // The host offers its network address when it has one, so it is bound to loopback or the PC has no network.
        return `This browser does not stream from this PC’s loopback address, ${offered.address}, the only address the host can offer. Use Chrome or Edge instead.`;
      }
      return `This browser did not try to reach the PC at ${where}. A browser setting or extension that limits WebRTC, or a VPN, usually causes this: turn it off for this page and try again.`;
    case 'no-answer':
      return `The PC did not answer at ${where}, which carries the picture and sound. Allow Rubylight or that port through the firewall on the PC, and check that no VPN on this device blocks it.`;
    case 'no-handshake':
      return 'This browser reached the PC, but the encrypted connection to it failed. Try again. If it keeps failing, antivirus or a firewall that inspects traffic may be in the way.';
    case 'lost':
      return 'The connection to the PC was lost. Check the network between this device and the PC, then start the stream again.';
  }
}

/** What the guest page says, to someone who may not run the PC. */
export function guestFailure(why: Failure, offered: Offered | null): string {
  const port = offered ? `UDP port ${offered.port}` : 'its media port';
  switch (why) {
    case 'no-try':
      return `Your browser did not try to reach the PC on ${port}. A browser setting or extension that limits WebRTC, or a VPN, usually causes this: turn it off for this page and join again.`;
    case 'no-answer':
      return `Your browser could not reach the PC on ${port}, which carries the picture and sound. Ask whoever invited you to forward that port to their PC, or to connect over Tailscale or a VPN. A firewall or VPN on your side can block it too.`;
    case 'no-handshake':
      return 'Your browser reached the PC, but the encrypted connection to it failed. Join again. If it keeps failing, antivirus or a firewall that inspects traffic may be in the way.';
    case 'lost':
      return 'The connection to the PC was lost. Check your network and join again.';
  }
}

/** The part of a peer connection the watch reads; a test passes a fake. */
export type Watched = {
  connectionState: string;
  onconnectionstatechange: ((event: Event) => void) | null;
  getStats(): Promise<{ forEach(visit: (stat: { type?: string; state?: string; responsesReceived?: number }) => void): void }>;
};

/**
 * Watches a peer connection from its answer on, until it closes. `connected` runs when it
 * connects. `failed` runs once, when the browser gives up, when `ms` passed without a single
 * answer, or when `handshakeMs` passed from the first answer without connecting; a connection that
 * is closed says nothing. Pairs are remembered as they appear, since Chrome drops them the moment
 * it gives up.
 */
export function watchConnection(pc: Watched, connected: () => void, failed: (why: Failure) => void, ms = NO_ANSWER_MS, every = LOOK_EVERY_MS, handshakeMs = NO_HANDSHAKE_MS) {
  const started = Date.now();
  let tried = false;
  let answered = false;
  let answeredAt = 0;
  let up = false;
  let over = false;
  let timer: ReturnType<typeof setTimeout> | undefined;

  const look = async () => {
    try {
      (await pc.getStats()).forEach((stat) => {
        if (stat.type !== 'candidate-pair') return;
        tried = true;
        if (stat.state === 'succeeded' || Number(stat.responsesReceived) > 0) answered = true;
      });
    } catch {
      // A connection closing as it is asked: what was seen before still stands.
    }
  };
  const stop = () => {
    over = true;
    clearTimeout(timer);
  };
  const fail = () => {
    if (over || pc.connectionState === 'closed') return stop();
    stop();
    failed(up ? 'lost' : answered ? 'no-handshake' : tried ? 'no-answer' : 'no-try');
  };

  pc.onconnectionstatechange = () => {
    if (over) return;
    if (pc.connectionState === 'connected') {
      up = true;
      clearTimeout(timer);
      connected();
    } else if (pc.connectionState === 'failed') {
      if (up) return fail();
      void look().then(fail);
    } else if (pc.connectionState === 'closed') stop();
  };
  // getStats asks the browser, so nothing here reaches the host.
  const tick = () => {
    if (pc.connectionState === 'closed') return stop();
    void look().then(() => {
      if (over || up) return;
      if (answered) answeredAt ||= Date.now();
      // A browser without connectionState never says the connection is up, so an answer is as far
      // as this can see, and a stream that may be playing isn't ended on a guess.
      if (answered && typeof pc.connectionState !== 'string') return stop();
      if (answered ? Date.now() - answeredAt >= handshakeMs : Date.now() - started >= ms) return fail();
      timer = setTimeout(tick, every);
    });
  };
  timer = setTimeout(tick, every);
}
