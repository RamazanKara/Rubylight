import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

// Why a browser's connection to the PC failed (issue 67): Firefox on a loopback candidate said
// only "The connection to the host failed." five seconds after the start.
globalThis.location = new URL('https://192.168.4.69:47990/');
globalThis.window = new EventTarget();
const server = await createTestServer();
const reach = await server.ssrLoadModule('/src/lib/webrtc/reach.ts');
const client = await server.ssrLoadModule('/src/lib/webrtc/client.ts');
const guest = await server.ssrLoadModule('/src/lib/webrtc/guest.ts');
const signalling = await server.ssrLoadModule('/src/lib/webrtc/signalling.ts');

const candidateLine = (address, port) => `a=candidate:fffeff7e32c583ef85edbb1a 1 udp 2130706175 ${address} ${port} typ host ufrag p2o7Yhc0YcQSAEox`;
const SDP = (address, port) => `v=0\r\na=ice-lite\r\n${candidateLine(address, port)}\r\na=mid:0\r\n`;

/** A peer connection the test drives: it changes state, and reports pairs, on demand. */
class FakePeer {
  static last = null;
  connectionState = 'new';
  onconnectionstatechange = null;
  pairs = [];
  localDescription = null;
  constructor() {
    FakePeer.last = this;
  }
  async getStats() {
    return this.pairs.map((state) => (typeof state === 'string' ? { type: 'candidate-pair', state } : { type: 'candidate-pair', ...state }));
  }
  become(state) {
    this.connectionState = state;
    this.onconnectionstatechange?.();
  }
  async setRemoteDescription() {}
  async createAnswer() {
    return { sdp: 'v=0\r\na=rtpmap:111 opus/48000/2\r\na=fmtp:111 minptime=10\r\n' };
  }
  async setLocalDescription(description) {
    this.localDescription = description;
  }
  close() {
    this.connectionState = 'closed';
  }
  createDataChannel() {
    return { close() {}, readyState: 'connecting' };
  }
}

const settle = (ms = 40) => new Promise((resolve) => setTimeout(resolve, ms));

function watching(pc, ms = 1000) {
  const seen = { connected: 0, failed: [] };
  reach.watchConnection(pc, () => seen.connected++, (why) => seen.failed.push(why), ms, 5);
  return seen;
}

test('the offer says where the browser has to send its media', () => {
  assert.deepEqual(reach.offeredAddress(SDP('192.168.4.69', 8001)), { address: '192.168.4.69', port: 8001 });
  assert.deepEqual(reach.offeredAddress(SDP('fd43::5', 8002)), { address: 'fd43::5', port: 8002 });
  assert.equal(reach.offeredAddress('v=0\r\n'), null);
});

test('a browser that gives up without a pair never tried, as Firefox on a loopback candidate', async () => {
  const pc = new FakePeer();
  const seen = watching(pc);
  pc.become('failed');
  await settle();
  assert.deepEqual(seen.failed, ['no-try']);
});

test('checks with no answer are called within seconds, before the browser gives up', async () => {
  const pc = new FakePeer();
  pc.pairs = ['in-progress'];
  const seen = watching(pc, 30);
  await settle(120);
  assert.deepEqual(seen.failed, ['no-answer']);
  // Said once, whatever the browser does afterwards.
  pc.become('failed');
  await settle();
  assert.deepEqual(seen.failed, ['no-answer']);
});

test('a browser that makes no pair at all is called too, as one whose policy forbids UDP', async () => {
  const seen = watching(new FakePeer(), 30);
  await settle(120);
  assert.deepEqual(seen.failed, ['no-try']);
});

test('pairs Chrome drops as it gives up still count as tried', async () => {
  const pc = new FakePeer();
  pc.pairs = [{ state: 'in-progress', responsesReceived: 0 }];
  const seen = watching(pc);
  await settle();
  pc.pairs = [];
  pc.become('failed');
  await settle();
  assert.deepEqual(seen.failed, ['no-answer']);
});

test('an answered check waits for the browser, and a failure after it is the handshake', async () => {
  const pc = new FakePeer();
  pc.pairs = [{ state: 'in-progress', responsesReceived: 1 }];
  const seen = watching(pc, 10);
  await settle(80);
  assert.deepEqual(seen.failed, []);
  pc.become('failed');
  await settle();
  assert.deepEqual(seen.failed, ['no-handshake']);
});

test('a slow start that connects says nothing, and a later failure is a lost connection', async () => {
  const pc = new FakePeer();
  pc.pairs = ['in-progress'];
  const seen = watching(pc, 30);
  pc.pairs = ['succeeded'];
  pc.become('connected');
  await settle(120);
  assert.equal(seen.connected, 1);
  assert.deepEqual(seen.failed, []);
  pc.become('failed');
  await settle();
  assert.deepEqual(seen.failed, ['lost']);
});

test('a connection the page closed says nothing', async () => {
  const pc = new FakePeer();
  const seen = watching(pc, 30);
  pc.close();
  await settle(120);
  assert.deepEqual(seen.failed, []);
});

test('the words name the cause, the port and the fix', () => {
  const lan = { address: '192.168.4.69', port: 8001 };
  const loop = { address: '127.0.0.1', port: 8001 };
  assert.match(reach.consoleFailure('no-try', loop), /loopback address, 127\.0\.0\.1.*only address.*Chrome or Edge/);
  assert.match(reach.consoleFailure('no-try', { address: '::1', port: 8001 }), /loopback/);
  assert.match(reach.consoleFailure('no-try', lan), /192\.168\.4\.69, UDP port 8001.*extension.*VPN/);
  assert.match(reach.consoleFailure('no-answer', lan), /did not answer at 192\.168\.4\.69, UDP port 8001.*firewall on the PC/);
  assert.match(reach.consoleFailure('no-handshake', lan), /encrypted connection/);
  assert.match(reach.consoleFailure('lost', lan), /was lost/);
  const seat = { address: '203.0.113.7', port: 8003 };
  assert.match(reach.guestFailure('no-try', seat), /UDP port 8003.*extension/);
  assert.match(reach.guestFailure('no-answer', seat), /UDP port 8003.*forward that port.*Tailscale/);
  assert.match(reach.guestFailure('no-handshake', seat), /encrypted connection/);
  assert.match(reach.guestFailure('lost', seat), /was lost/);
  // A guest's text doesn't send them to a firewall on a PC they cannot touch, or show the address.
  assert.doesNotMatch(reach.guestFailure('no-answer', seat), /203\.0\.113\.7/);
  for (const why of ['no-try', 'no-answer']) {
    assert.ok(reach.consoleFailure(why, null));
    assert.ok(reach.guestFailure(why, null));
  }
});

/** The console's socket, driven by the test. */
class FakeSocket {
  static last = null;
  constructor() {
    this.protocol = '';
    this.sent = [];
    FakeSocket.last = this;
  }
  send(data) {
    this.sent.push(JSON.parse(data));
  }
  close() {}
  receive(message) {
    this.onmessage?.({ data: JSON.stringify({ version: 2, ...message }) });
  }
}

const CAPABILITIES = {
  type: 'capabilities',
  videoModes: [{ width: 1920, height: 1080, frameRates: [60], codecs: ['h264'], defaultBitrateKbps: 20000 }],
  extensions: { input: { label: 'input', version: 1, keyboard: true, mouse: true } },
};

function browserGlobals() {
  globalThis.WebSocket = FakeSocket;
  globalThis.RTCPeerConnection = FakePeer;
  globalThis.MediaStream = class {
    addTrack() {}
  };
  globalThis.RTCRtpReceiver = { getCapabilities: () => ({ codecs: [{ mimeType: 'video/H264' }] }) };
}

async function consoleStream() {
  browserGlobals();
  const opening = client.ConsoleStream.connect('token', 'wss://pc/api/webrtc/signal');
  const socket = FakeSocket.last;
  socket.protocol = signalling.PROTOCOL;
  socket.onopen?.();
  await settle(0);
  socket.receive({ type: 'authenticated' });
  await settle(0);
  socket.receive(CAPABILITIES);
  return { stream: await opening, socket };
}

test('the Stream page says why, tells the host to stop, and leaves the reason in its log', async () => {
  const { stream, socket } = await consoleStream();
  const ended = [];
  stream.onended = (reason, closed) => ended.push({ reason, closed });
  const choice = { codec: 'h264', width: 1920, height: 1080, fps: 60 };
  const started = stream.start('7', choice, { play: async () => {}, srcObject: null }, () => {});
  await settle(0);
  socket.receive({ type: 'offer', sessionId: 's', sdp: SDP('127.0.0.1', 8001) });
  await settle();
  assert.ok(socket.sent.some((message) => message.type === 'answer'));
  FakePeer.last.become('failed');
  await assert.rejects(started, /loopback address, 127\.0\.0\.1/);
  assert.equal(ended.length, 1);
  assert.match(ended[0].reason, /loopback address, 127\.0\.0\.1/);
  assert.equal(ended[0].closed, false);
  const types = socket.sent.map((message) => message.type);
  assert.ok(types.includes('stop-session'), types.join());
  const log = socket.sent.find((message) => message.type === 'client-log');
  assert.match(log.lines[0], /connection failed: no-try, offered 127\.0\.0\.1:8001/);
});

test('a guest’s page says why too, and offers to join again', async () => {
  browserGlobals();
  const endings = [];
  let socket;
  const stream = guest.GuestStream.join('0'.repeat(32), 'Sam', new MediaStream(), () => assert.fail('no stream'), 'wss://pc/guest/signal', (url, protocols) => {
    socket = new FakeSocket(url, protocols);
    return socket;
  });
  stream.onended = (why) => endings.push(why);
  socket.protocol = guest.GUEST_PROTOCOL;
  socket.onopen();
  socket.onmessage({ data: JSON.stringify({ type: 'offer', sdp: SDP('192.168.4.69', 8002), name: 'Sam', permission: 'view' }) });
  await settle();
  assert.deepEqual(endings, []);
  assert.ok(socket.sent.some((message) => message.type === 'answer'));
  FakePeer.last.pairs = ['in-progress'];
  await settle(1100);
  FakePeer.last.become('failed');
  await settle();
  assert.equal(endings.length, 1);
  assert.equal(endings[0].code, 'connection-lost');
  assert.match(endings[0].message, /UDP port 8002.*forward that port/);
  assert.equal(endings[0].final, false);
});
