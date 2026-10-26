// Protocol v2 signalling for the Stream page, over the console's own socket
// `/api/webrtc/signal` (docs/webrtc.md, "Stream in the console"). Ported from
// rust/tests/webrtc_client/signalling.js. The console sign-in authenticates the
// socket: the host answers with `authenticated` and `capabilities` at once, so
// there is no pairing and no proof here.

/** The subprotocol the host answers with. */
export const PROTOCOL = 'rubylight.signal.v2';

export type Message = { version?: number; type: string; [field: string]: unknown };

/** The signalling socket of the console this page came from. */
export function signalUrl(page: { protocol: string; host: string }): string {
  return `${page.protocol === 'https:' ? 'wss' : 'ws'}://${page.host}/api/webrtc/signal`;
}

/** The session's CSRF token travels as a subprotocol: a browser cannot set headers on a socket. */
export function subprotocols(csrf: string): string[] {
  return [PROTOCOL, `csrf.${csrf}`];
}

/** A refused socket, an `error` message from the host, or a missing reply. */
export class SignallingError extends Error {
  constructor(
    message: string,
    readonly code = '',
  ) {
    super(message);
  }
}

/** The host refused the upgrade. A browser hides the HTTP status, so this is all the page learns. */
export class RefusedError extends SignallingError {}

/** Unawaited messages kept for a later `expect`; status updates during a stream pile up otherwise. */
const MAX_QUEUED = 64;

type Waiter = {
  match: (message: Message) => boolean;
  what: string;
  resolve: (message: Message) => void;
  reject: (error: Error) => void;
  timer: ReturnType<typeof setTimeout>;
};

export class Signalling {
  private queue: Message[] = [];
  private waiters: Waiter[] = [];
  private listeners: ((message: Message) => void)[] = [];
  closed = false;
  /** Why the host closed the socket, such as "signed out". */
  closeReason = '';
  onclose: (() => void) | null = null;

  private constructor(private ws: WebSocket) {}

  /** Opens the socket; rejects with a RefusedError when the host refuses it. */
  static open(url: string, csrf: string, timeout = 15000): Promise<Signalling> {
    return new Promise((resolve, reject) => {
      const ws = new WebSocket(url, subprotocols(csrf));
      const signalling = new Signalling(ws);
      let settled = false;
      const fail = (error: Error) => {
        if (settled) return;
        settled = true;
        clearTimeout(timer);
        reject(error);
      };
      const timer = setTimeout(() => {
        ws.close();
        fail(new SignallingError('The host did not answer the stream socket in time.'));
      }, timeout);
      ws.onopen = () => {
        if (ws.protocol !== PROTOCOL) {
          ws.close();
          fail(new SignallingError(`The host answered with the subprotocol "${ws.protocol}", not ${PROTOCOL}.`));
          return;
        }
        settled = true;
        clearTimeout(timer);
        resolve(signalling);
      };
      ws.onerror = () => fail(new RefusedError('The host refused the stream socket.'));
      ws.onclose = (event) => {
        fail(new RefusedError('The host refused the stream socket.'));
        signalling.closed = true;
        signalling.closeReason = event.reason;
        signalling.flush();
        signalling.onclose?.();
      };
      ws.onmessage = (event) => {
        let message: Message;
        try {
          message = JSON.parse(String(event.data)) as Message;
        } catch {
          console.error('The host sent a message that is not JSON:', String(event.data).slice(0, 200));
          return;
        }
        for (const listener of signalling.listeners) listener(message);
        signalling.queue.push(message);
        if (signalling.queue.length > MAX_QUEUED) signalling.queue.shift();
        signalling.flush();
      };
    });
  }

  send(type: string, fields: Record<string, unknown> = {}) {
    if (this.closed) {
      throw new SignallingError(this.closeReason ? `The host closed the stream socket: ${this.closeReason}.` : 'The stream socket is closed.');
    }
    this.ws.send(JSON.stringify({ version: 2, type, ...fields }));
  }

  close() {
    this.ws.close();
  }

  /** Forgets the messages nobody waited for, so a stale `error` cannot answer the next request. */
  clear() {
    this.queue = [];
  }

  /** Every message from now on; returns a function that stops listening. */
  on(listener: (message: Message) => void): () => void {
    this.listeners.push(listener);
    return () => {
      this.listeners = this.listeners.filter((l) => l !== listener);
    };
  }

  private flush() {
    for (const waiter of this.waiters.slice()) {
      const index = this.queue.findIndex(waiter.match);
      if (index >= 0) {
        this.drop(waiter);
        waiter.resolve(this.queue.splice(index, 1)[0]!);
      } else if (this.closed) {
        this.drop(waiter);
        const reason = this.closeReason ? ` (${this.closeReason})` : '';
        waiter.reject(new SignallingError(`The stream socket closed while waiting for ${waiter.what}${reason}.`));
      }
    }
  }

  private drop(waiter: Waiter) {
    this.waiters.splice(this.waiters.indexOf(waiter), 1);
    clearTimeout(waiter.timer);
  }

  /**
   * The next message of `type`, or of any of `types`. An `error` message rejects
   * unless it is wanted.
   */
  expect(type: string | string[], timeout = 15000): Promise<Message> {
    const types = Array.isArray(type) ? type : [type];
    return new Promise((resolve, reject) => {
      const waiter: Waiter = {
        match: (m) => types.includes(m.type) || m.type === 'error',
        what: types.join(' or '),
        resolve: (message) => {
          if (message.type === 'error' && !types.includes('error')) {
            reject(new SignallingError(String(message.message ?? message.code), String(message.code ?? '')));
          } else resolve(message);
        },
        reject,
        timer: setTimeout(() => {
          this.drop(waiter);
          reject(new SignallingError(`The host did not send ${waiter.what} in time.`));
        }, timeout),
      };
      this.waiters.push(waiter);
      this.flush();
    });
  }
}
