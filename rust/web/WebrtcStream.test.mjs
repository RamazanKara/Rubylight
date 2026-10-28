import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

globalThis.location = new URL('http://localhost/');
globalThis.window = new EventTarget();
const server = await createTestServer();
const input = await server.ssrLoadModule('/src/lib/webrtc/input.ts');
const client = await server.ssrLoadModule('/src/lib/webrtc/client.ts');
const signalling = await server.ssrLoadModule('/src/lib/webrtc/signalling.ts');
const { default: Stream } = await server.ssrLoadModule('/src/pages/Stream.svelte');
const { render } = await server.ssrLoadModule('svelte/server');

/** A WebSocket the test drives: it opens, answers and closes on demand. */
class FakeSocket {
  static last = null;
  constructor(url, protocols) {
    this.url = url;
    this.protocols = protocols;
    this.protocol = '';
    this.sent = [];
    FakeSocket.last = this;
  }
  send(data) {
    this.sent.push(JSON.parse(data));
  }
  close() {}
  open(protocol = signalling.PROTOCOL) {
    this.protocol = protocol;
    this.onopen?.();
  }
  receive(message) {
    this.onmessage?.({ data: JSON.stringify({ version: 2, ...message }) });
  }
  hangUp(reason = '') {
    this.onclose?.({ reason });
  }
}
globalThis.WebSocket = FakeSocket;

const CAPABILITIES = {
  type: 'capabilities',
  videoModes: [
    { width: 1280, height: 720, frameRates: [30, 60], codecs: ['h264'], defaultBitrateKbps: 10000 },
    { width: 1920, height: 1080, frameRates: [30, 60, 120], codecs: ['h264', 'hevc'], defaultBitrateKbps: 20000 },
  ],
  extensions: {
    input: { label: 'input', version: 1, keyboard: true, mouse: true },
    codecFrameRates: [{ width: 1920, height: 1080, codec: 'h264', frameRates: [30, 60] }],
  },
};

test('the socket is the console host’s, with the CSRF token as a subprotocol', () => {
  assert.equal(signalling.signalUrl({ protocol: 'https:', host: '192.168.1.5:47990' }), 'wss://192.168.1.5:47990/api/webrtc/signal');
  assert.equal(signalling.signalUrl({ protocol: 'http:', host: 'localhost:5173' }), 'ws://localhost:5173/api/webrtc/signal');
  assert.deepEqual(signalling.subprotocols('abc'), ['rubylight.signal.v2', 'csrf.abc']);
});

test('keys travel by DOM code and buttons 0 to 4 only', () => {
  assert.deepEqual(input.keyMessage('KeyA', true), { v: 1, type: 'key', code: 'KeyA', down: true });
  assert.deepEqual(input.keyMessage('F24', false), { v: 1, type: 'key', code: 'F24', down: false });
  assert.equal(input.keyMessage('', true), null);
  assert.deepEqual(input.buttonMessage(2, true), { v: 1, type: 'mouse-button', button: 2, down: true });
  assert.deepEqual(input.buttonMessage(4, false), { v: 1, type: 'mouse-button', button: 4, down: false });
  for (const button of [-1, 5, 1.5, NaN]) assert.equal(input.buttonMessage(button, true), null, String(button));
});

test('the pointer position leaves out the letterbox bars', () => {
  // 1920×1080 in a 1000×1000 box at (100, 50): 1000×562.5, with 218.75 px bars above and below.
  const box = { left: 100, top: 50, width: 1000, height: 1000 };
  const video = { width: 1920, height: 1080 };
  const at = (x, y) => input.positionMessage(x, y, box, video);
  assert.deepEqual(at(600, 550), { v: 1, type: 'mouse-position', x: 0.5, y: 0.5 });
  const quarter = at(350, 50 + 218.75 + 562.5 / 4);
  assert.ok(Math.abs(quarter.x - 0.25) < 1e-9 && Math.abs(quarter.y - 0.25) < 1e-9, JSON.stringify(quarter));
  // On a bar or outside the box: clamped to the picture's edge.
  assert.deepEqual(at(90, 60), { v: 1, type: 'mouse-position', x: 0, y: 0 });
  assert.deepEqual(at(2000, 2000), { v: 1, type: 'mouse-position', x: 1, y: 1 });
  // Pillarbox: 4:3 in a 16:9 box.
  const pillar = input.positionMessage(240, 0, { left: 0, top: 0, width: 1920, height: 1080 }, { width: 1440, height: 1080 });
  assert.deepEqual(pillar, { v: 1, type: 'mouse-position', x: 0, y: 0 });
  // No frame yet, or a hidden element.
  assert.equal(input.positionMessage(10, 10, box, { width: 0, height: 0 }), null);
  assert.equal(input.positionMessage(10, 10, { left: 0, top: 0, width: 0, height: 0 }, video), null);
});

test('a second button pressed or released while one is held is not lost', () => {
  const button = (button, down) => ({ v: 1, type: 'mouse-button', button, down });
  // Left down, then right while left is held: the browser reports the right one as a move.
  assert.deepEqual(input.buttonChanges(0, 1), [button(0, true)]);
  assert.deepEqual(input.buttonChanges(1, 3), [button(2, true)]);
  assert.deepEqual(input.buttonChanges(3, 2), [button(0, false)]);
  assert.deepEqual(input.buttonChanges(2, 0), [button(2, false)]);
  // The mask's middle and right bits are swapped against the DOM button numbers.
  assert.deepEqual(input.buttonChanges(0, 4), [button(1, true)]);
  assert.deepEqual(input.buttonChanges(0, 8 | 16), [button(3, true), button(4, true)]);
  // A cancelled pointer lets go of everything still down.
  assert.deepEqual(input.buttonChanges(31, 0), [0, 1, 2, 3, 4].map((b) => button(b, false)));
  assert.deepEqual(input.buttonChanges(5, 5), []);
});

test('one wheel notch is 120 units in every delta mode, with DOM signs', () => {
  const once = (...args) => new input.Wheel().message(...args);
  assert.deepEqual(once(0, 100, 0), { v: 1, type: 'wheel', dx: 0, dy: 120 });
  assert.deepEqual(once(0, -3, 1), { v: 1, type: 'wheel', dx: 0, dy: -120 });
  assert.deepEqual(once(1, 0, 2), { v: 1, type: 'wheel', dx: 360, dy: 0 });
  assert.deepEqual(once(-50, 25, 0), { v: 1, type: 'wheel', dx: -60, dy: 30 });
  assert.equal(once(0, 0, 0), null);
  assert.equal(once(0.1, 0.2, 0), null);
});

test('a touchpad’s small wheel deltas add up instead of vanishing', () => {
  const wheel = new input.Wheel();
  // 0.5 px is 0.6 units: nothing until a whole unit has built up, and nothing is lost.
  const sent = [];
  for (let i = 0; i < 200; i++) {
    const message = wheel.message(-0.25, 0.5, 0);
    if (message) sent.push(message);
  }
  assert.equal(sent.reduce((sum, m) => sum + m.dy, 0), 120);
  assert.equal(sent.reduce((sum, m) => sum + m.dx, 0), -60);
  assert.ok(sent.every((m) => Number.isInteger(m.dx) && Number.isInteger(m.dy)));
  // Ten tenths of a line are one line, 40 units.
  const lines = new input.Wheel();
  let dy = 0;
  for (let i = 0; i < 10; i++) dy += lines.message(0, 0.1, 1)?.dy ?? 0;
  assert.equal(dy, 40);
});

test('releasing everything lets go of exactly what is still down, once', () => {
  const held = new input.Held();
  for (const message of [
    input.keyMessage('KeyW', true),
    input.keyMessage('ShiftLeft', true),
    input.buttonMessage(0, true),
    input.buttonMessage(2, true),
    input.keyMessage('ShiftLeft', false),
    input.buttonMessage(2, false),
    new input.Wheel().message(0, 100, 0),
  ]) held.track(message);
  assert.deepEqual(held.releaseAll(), [
    { v: 1, type: 'key', code: 'KeyW', down: false },
    { v: 1, type: 'mouse-button', button: 0, down: false },
  ]);
  assert.deepEqual(held.releaseAll(), []);
});

const ANSWER = [
  'v=0',
  'm=audio 9 UDP/TLS/RTP/SAVPF 111 0',
  'a=rtpmap:111 opus/48000/2',
  'a=fmtp:111 minptime=10;useinbandfec=1',
  'a=rtpmap:0 PCMU/8000',
  'm=video 9 UDP/TLS/RTP/SAVPF 96',
  'a=rtpmap:96 H264/90000',
  'a=fmtp:96 level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42e01f',
  '',
].join('\r\n');

test('the answer asks for stereo Opus on the Opus format only', () => {
  const munged = client.stereoAnswer(ANSWER);
  assert.match(munged, /^a=fmtp:111 minptime=10;useinbandfec=1;stereo=1;sprop-stereo=1\r$/m);
  assert.match(munged, /^a=fmtp:96 level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42e01f\r$/m);
  assert.equal(client.stereoAnswer(munged), munged, 'munging twice changes nothing');
  // Another payload type, and no fmtp line at all.
  const bare = ANSWER.replaceAll('111', '109').replace('a=fmtp:109 minptime=10;useinbandfec=1\r\n', '');
  assert.match(client.stereoAnswer(bare), /a=rtpmap:109 opus\/48000\/2\r\na=fmtp:109 stereo=1;sprop-stereo=1\r\na=rtpmap:0 /);
  assert.throws(() => client.stereoAnswer(ANSWER.replace('opus/48000/2', 'opus/48000/1')), /without stereo Opus/);
  assert.throws(() => client.stereoAnswer(''), /without stereo Opus/);
});

test('the page streams H.264 1080p60 at the host’s bitrate, or says why not', () => {
  assert.equal(client.streamBitrate(CAPABILITIES), 20000);
  // Without codecFrameRates, the mode's own codecs and rates decide.
  const plain = { ...CAPABILITIES, extensions: { input: CAPABILITIES.extensions.input } };
  assert.equal(client.streamBitrate(plain), 20000);
  const no60 = structuredClone(CAPABILITIES);
  no60.extensions.codecFrameRates[0].frameRates = [30];
  assert.throws(() => client.streamBitrate(no60), /does not offer H\.264 at 1920×1080 and 60 fps/);
  const noH264 = structuredClone(plain);
  noH264.videoModes[1].codecs = ['hevc'];
  assert.throws(() => client.streamBitrate(noH264), /does not offer H\.264/);
  assert.throws(() => client.streamBitrate({ ...CAPABILITIES, videoModes: [CAPABILITIES.videoModes[0]] }), /does not offer/);
  assert.throws(() => client.streamBitrate({ videoModes: CAPABILITIES.videoModes }), /keyboard and mouse/);
  const noMouse = structuredClone(CAPABILITIES);
  noMouse.extensions.input.mouse = false;
  assert.throws(() => client.streamBitrate(noMouse), /keyboard and mouse/);

  assert.equal(client.receivesH264({ codecs: [{ mimeType: 'video/VP8' }, { mimeType: 'video/H264' }], headerExtensions: [] }), true);
  assert.equal(client.receivesH264({ codecs: [{ mimeType: 'video/VP8' }], headerExtensions: [] }), false);
  assert.equal(client.receivesH264(null), false);
});

test('a refused socket is told apart from a host error', async () => {
  const refused = signalling.Signalling.open('wss://host/api/webrtc/signal', 'token');
  FakeSocket.last.onerror();
  FakeSocket.last.hangUp();
  await assert.rejects(refused, signalling.RefusedError);

  const wrong = signalling.Signalling.open('wss://host/api/webrtc/signal', 'token');
  FakeSocket.last.open('');
  await assert.rejects(wrong, (error) => !(error instanceof signalling.RefusedError) && /subprotocol/.test(error.message));
});

async function openStream() {
  const opening = client.ConsoleStream.connect('token', 'wss://host/api/webrtc/signal');
  const socket = FakeSocket.last;
  socket.open();
  socket.receive({ type: 'authenticated' });
  socket.receive(CAPABILITIES);
  return { stream: await opening, socket };
}

test('the console socket lists apps, and a stale error does not answer the next request', async () => {
  const { stream, socket } = await openStream();
  assert.deepEqual(socket.protocols, ['rubylight.signal.v2', 'csrf.token']);
  assert.equal(stream.capabilities.videoModes.length, 2);

  // An error nobody waited for, such as one about an earlier request.
  socket.receive({ type: 'error', requestType: 'get-apps', code: 'internal', message: 'Stale' });
  const listing = stream.apps();
  assert.deepEqual(socket.sent.at(-1), { version: 2, type: 'get-apps' });
  // An error about another request, while this one waits, is not its answer either.
  socket.receive({ type: 'error', requestType: 'answer', code: 'session-not-found', message: 'No session' });
  socket.receive({ type: 'apps', apps: [{ id: '1', title: 'Desktop', artworkAvailable: false, running: false }] });
  assert.deepEqual((await listing).map((app) => app.title), ['Desktop']);

  // A host error for the request itself still rejects it.
  const failing = stream.apps();
  socket.receive({ type: 'error', requestType: 'get-apps', code: 'internal', message: 'Library unavailable' });
  await assert.rejects(failing, /Library unavailable/);

  // Another tab takes the socket over, or the user signs out: the page hears why, and that the socket is gone.
  const ended = new Promise((resolve) => (stream.onended = (...args) => resolve(args)));
  socket.hangUp('Opened in another tab or browser');
  assert.deepEqual(await ended, ['The host closed the stream connection: Opened in another tab or browser.', true]);
  assert.equal(stream.closed, true);
  await assert.rejects(stream.apps(), /another tab/);
  assert.equal(client.closedMessage(''), 'The host closed the stream connection.');
});

test('quitting the app waits for the host to say it stopped, and says why it did not', async () => {
  const { stream, socket } = await openStream();
  const quitting = stream.quit();
  assert.deepEqual(socket.sent.at(-1), { version: 2, type: 'stop-host-session' });
  let done = false;
  quitting.then(() => (done = true));
  socket.receive({ type: 'host-session-status', state: 'stopping' });
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.equal(done, false, 'stopping is not stopped');
  socket.receive({ type: 'host-session-status', state: 'stopped' });
  await quitting;

  const failed = stream.quit();
  socket.receive({ type: 'host-session-status', state: 'stopping' });
  socket.receive({ type: 'host-session-status', state: 'failed', message: 'A stream did not stop in time' });
  await assert.rejects(failed, /The host did not quit the app: A stream did not stop in time\./);

  const refused = stream.quit();
  socket.receive({ type: 'error', requestType: 'stop-host-session', code: 'forbidden', message: 'Another client started the app' });
  await assert.rejects(refused, /Another client started the app/);
});

test('a lapsed sign-in’s anonymous token does not replace the session’s', async () => {
  const api = await server.ssrLoadModule('/src/lib/api.ts');
  const realFetch = globalThis.fetch;
  const headers = [];
  globalThis.fetch = async (url, init = {}) => {
    headers.push([String(url), new Headers(init.headers).get('X-CSRF-Token')]);
    const body = String(url).endsWith('/api/csrf-token') ? { csrf_token: 'anonymous' } : { status: true };
    return new Response(JSON.stringify(body), { status: 200, headers: { 'Content-Type': 'application/json' } });
  };
  try {
    api.setCsrfToken('session');
    assert.equal(await api.sessionCsrfToken(), 'anonymous');
    await api.api.auth.logout();
    assert.deepEqual(headers.at(-1), ['/api/auth/logout', 'session']);
  } finally {
    globalThis.fetch = realFetch;
  }
});

test('the page carries no Experimental badge and has the stream video the e2e probe finds', () => {
  const { body } = render(Stream);
  assert.doesNotMatch(body, />Experimental</);
  assert.match(body, /<video id="stream-video"/);
  // The socket opens on Connect, not with the page.
  assert.match(body, /Checking the host/);
});
