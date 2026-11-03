import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

// Guests of the Stream page (docs/browser-guests.md): the owner's links and the guest page's client.
globalThis.location = new URL('https://192.168.4.69:47990/guest/');
globalThis.window = new EventTarget();
const server = await createTestServer();
const guests = await server.ssrLoadModule('/src/lib/guests.ts');
const guest = await server.ssrLoadModule('/src/lib/webrtc/guest.ts');
const gamepad = await server.ssrLoadModule('/src/lib/webrtc/gamepad.ts');
const { default: GuestPanel } = await server.ssrLoadModule('/src/components/stream/GuestPanel.svelte');
const { default: Guest } = await server.ssrLoadModule('/src/guest/Guest.svelte');
const { render } = await server.ssrLoadModule('svelte/server');

const TOKEN = '0123456789abcdef0123456789abcdef';

test('a link lasts one hour, a day, or five minutes to seven days', () => {
  assert.equal(guests.lifetime('1h', 0, 0), 3600);
  assert.equal(guests.lifetime('24h', 0, 0), 86_400);
  assert.equal(guests.lifetime('custom', 2, 3600), 7200);
  assert.equal(guests.lifetime('custom', 5, 60), 300);
  assert.equal(guests.lifetime('custom', 4, 60), null);
  assert.equal(guests.lifetime('custom', 7, 86_400), 7 * 86_400);
  assert.equal(guests.lifetime('custom', 8, 86_400), null);
  assert.equal(guests.lifetime('custom', Number(''), 60), null);
  assert.equal(guests.lifetime('custom', Number('x'), 60), null);
});

test('the link carries the token in its fragment, at an address a guest can open', () => {
  assert.equal(guests.guestLink('192.168.4.69:47990', TOKEN), `https://192.168.4.69:47990/guest/#${TOKEN}`);
  // A console opened on this PC gives the PC's LAN address instead of localhost.
  const page = (host) => {
    const url = new URL(`https://${host}/`);
    return { hostname: url.hostname, host: url.host, port: url.port };
  };
  assert.equal(guests.linkAddress(page('localhost:47990'), ['192.168.4.69']), '192.168.4.69:47990');
  assert.equal(guests.linkAddress(page('127.0.0.1:47990'), ['192.168.4.69']), '192.168.4.69:47990');
  assert.equal(guests.linkAddress(page('[::1]:47990'), ['192.168.4.69']), '192.168.4.69:47990');
  assert.equal(guests.linkAddress(page('pc.local:47990'), ['192.168.4.69']), 'pc.local:47990');
  assert.equal(guests.linkAddress(page('localhost:47990'), []), 'localhost:47990');
  for (const good of ['192.168.4.69:47990', 'home.example.org', '[2001:db8::1]:47990']) assert.ok(guests.validAddress(good), good);
  for (const bad of ['', 'a b', 'host/path', 'https://host', 'host:port', 'host#x']) assert.ok(!guests.validAddress(bad), bad);
});

test('expiry reads in minutes, hours or days', () => {
  assert.equal(guests.expiresIn(1000 + 59 * 60, 1000), 'in 59 min');
  assert.equal(guests.expiresIn(1000 + 10, 1000), 'in 1 min');
  assert.equal(guests.expiresIn(1000 + 23 * 3600, 1000), 'in 23 h');
  assert.equal(guests.expiresIn(1000 + 6 * 86_400, 1000), 'in 6 days');
  assert.equal(guests.permissionLabel('controller-keyboard-mouse'), 'Controller, keyboard and mouse');
});

test('the guest page takes the token from the fragment only in its exact form', () => {
  assert.equal(guest.inviteToken(`#${TOKEN}`), TOKEN);
  assert.equal(guest.inviteToken(`#${TOKEN.toUpperCase()}`), TOKEN);
  for (const bad of ['', '#', `#${TOKEN.slice(1)}`, `#${TOKEN}0`, `#${TOKEN.slice(1)}g`]) assert.equal(guest.inviteToken(bad), null, bad);
  assert.equal(guest.guestSignalUrl({ protocol: 'https:', host: 'pc:47990' }), 'wss://pc:47990/guest/signal');
});

test('only a link that still works waits, and a withdrawn or wrong one is final', () => {
  const wait = guest.ending({ type: 'error', code: 'not-streaming', message: 'The host is not streaming right now.' });
  assert.deepEqual(wait, { code: 'not-streaming', message: 'The host is not streaming right now.', wait: true, final: false });
  assert.equal(guest.ending({ type: 'ended', reason: 'ended', message: 'x' }).wait, true);
  assert.equal(guest.ending({ type: 'ended', reason: 'revoked', message: 'x' }).final, true);
  assert.equal(guest.ending({ type: 'error', code: 'invalid-link', message: 'x' }).final, true);
  const kicked = guest.ending({ type: 'ended', reason: 'kicked', message: 'The host removed you from the stream.' });
  assert.equal(kicked.wait || kicked.final, false);
  assert.equal(guest.ending({ type: 'error', code: 'full', message: 'x' }).final, false);
  assert.deepEqual(guest.ending({}), { code: 'connection-lost', message: 'The connection to the host was lost.', wait: false, final: false });
  assert.ok(!guest.gamepads('view') && !guest.keyboardMouse('view'));
  assert.ok(guest.gamepads('controller') && !guest.keyboardMouse('controller'));
  assert.ok(guest.gamepads('controller-keyboard-mouse') && guest.keyboardMouse('controller-keyboard-mouse'));
});

/** A WebSocket the test drives. */
class FakeSocket {
  constructor(url, protocols) {
    this.url = url;
    this.protocols = protocols;
    this.protocol = '';
    this.sent = [];
    this.closed = false;
  }
  send(data) {
    this.sent.push(JSON.parse(data));
  }
  close() {
    this.closed = true;
  }
  open(protocol = guest.GUEST_PROTOCOL) {
    this.protocol = protocol;
    this.onopen?.();
  }
  say(message) {
    this.onmessage?.({ data: JSON.stringify(message) });
  }
}

function joining() {
  let socket;
  const endings = [];
  const stream = guest.GuestStream.join(TOKEN, 'Sam', {}, () => assert.fail('no stream'), 'wss://pc/guest/signal', (url, protocols) => (socket = new FakeSocket(url, protocols)));
  stream.onended = (why) => endings.push(why);
  return { stream, socket, endings };
}

test('the page sends its token and name first, on the guest subprotocol', () => {
  const { socket, endings } = joining();
  assert.deepEqual(socket.protocols, ['rubylight.guest.v1']);
  assert.deepEqual(socket.sent, []);
  socket.open();
  assert.deepEqual(socket.sent, [{ type: 'join', token: TOKEN, name: 'Sam' }]);
  socket.say({ type: 'error', code: 'not-streaming', message: 'The host is not streaming right now.' });
  assert.equal(endings.length, 1);
  assert.equal(endings[0].wait, true);
  assert.ok(socket.closed);
  // The close after the message says nothing more.
  socket.onclose?.({});
  assert.equal(endings.length, 1);
});

test('a host that answers with another subprotocol gets no token', () => {
  const { socket, endings } = joining();
  socket.open('rubylight.signal.v2');
  assert.deepEqual(socket.sent, []);
  assert.equal(endings[0].code, 'protocol');
});

test('a refused socket, a revoked link and a lost connection each say so once', () => {
  let run = joining();
  run.socket.onerror?.({});
  run.socket.onclose?.({});
  assert.deepEqual(run.endings.map((e) => e.code), ['refused']);
  run = joining();
  run.socket.open();
  run.socket.say({ type: 'ended', reason: 'revoked', message: 'The host withdrew this invite link.' });
  assert.deepEqual(run.endings.map((e) => [e.code, e.final]), [['revoked', true]]);
  run = joining();
  run.socket.open();
  run.socket.onclose?.({});
  assert.deepEqual(run.endings.map((e) => e.code), ['connection-lost']);
});

test('leaving tells the host and ends without a message', () => {
  const { stream, socket, endings } = joining();
  socket.open();
  stream.leave();
  assert.deepEqual(socket.sent.at(-1), { type: 'leave' });
  assert.ok(socket.closed);
  assert.deepEqual(endings, []);
});

/** A data channel that is open and keeps what it was sent. */
const channel = () => ({ readyState: 'open', sent: [], send(data) { this.sent.push(JSON.parse(data)); } });

test('pads go out only with a controller permission, keys only with keyboard and mouse', async () => {
  for (const [permission, pads, keys] of [
    ['view', false, false],
    ['controller', true, false],
    ['controller-keyboard-mouse', true, true],
  ]) {
    const { stream, socket } = joining();
    socket.open();
    const [control, gamepads] = [channel(), channel()];
    let opened = 0;
    // The host's two channels are there for every guest; the page's `input` channel is not.
    Object.assign(stream, {
      joined: { name: 'Sam', permission, codec: 'AV1', width: 1920, height: 1080, fps: 60 },
      control,
      pad: gamepads,
      pc: {
        createDataChannel(label, options) {
          opened++;
          assert.equal(label, 'input');
          assert.equal(options.ordered, true);
          const input = channel();
          queueMicrotask(() => input.onopen());
          return input;
        },
      },
    });
    await stream.streaming();
    assert.equal(opened, keys ? 1 : 0, permission);
    stream.pollPads([pad()], 0);
    assert.equal(control.sent.length + gamepads.sent.length > 0, pads, permission);
    stream.send({ v: 1, type: 'key', code: 'KeyA', down: true });
    assert.equal(stream.input?.sent.length ?? 0, keys ? 1 : 0, permission);
  }
});

function pad(overrides = {}) {
  const buttons = Array.from({ length: 17 }, () => ({ pressed: false, value: 0 }));
  return { index: 0, id: 'Xbox 360 Controller (STANDARD GAMEPAD)', mapping: 'standard', buttons, axes: [0, 0, 0, 0], ...overrides };
}

test('pad buttons map to the host’s bits, triggers and sticks as the Gamepad API reads them', () => {
  // Gamepad API index and the bit rust/core/src/webrtc/gamepad.rs expects.
  const bits = { 0: 0, 1: 1, 2: 2, 3: 3, 4: 4, 5: 5, 8: 6, 9: 7, 10: 8, 11: 9, 12: 10, 13: 11, 14: 12, 15: 13, 16: 14 };
  for (const [index, bit] of Object.entries(bits)) {
    const buttons = pad().buttons;
    buttons[index] = { pressed: true, value: 1 };
    assert.equal(gamepad.stateMessage(pad({ buttons }), 1).buttons, 1 << bit, `button ${index}`);
  }
  const buttons = pad().buttons;
  buttons[6] = { pressed: true, value: 0.25 };
  buttons[7] = { pressed: false, value: 0.5 };
  const state = gamepad.stateMessage(pad({ buttons, axes: [0.5, -1, 0.25, 1] }), 7);
  assert.deepEqual(state, {
    v: 2,
    type: 'gamepad-state',
    controllerId: 0,
    seq: 7,
    buttons: 0,
    leftTrigger: 0.25,
    rightTrigger: 0.5,
    leftStickX: 0.5,
    leftStickY: -1,
    rightStickX: 0.25,
    rightStickY: 1,
  });
});

test('pads are announced once, sent on change or every 250 ms, and unplugged when gone', () => {
  const pads = new gamepad.Pads();
  let out = pads.poll([pad()], 0);
  assert.deepEqual(out.control.map((m) => m.type), ['gamepad-connected']);
  assert.equal(out.control[0].buttons, 17);
  assert.equal(out.gamepad.length, 1);
  assert.equal(pads.poll([pad()], 100).gamepad.length, 0, 'unchanged and not due');
  const buttons = pad().buttons;
  buttons[0] = { pressed: true, value: 1 };
  out = pads.poll([pad({ buttons })], 120);
  assert.deepEqual(out.control, []);
  assert.equal(out.gamepad[0].buttons, 1);
  assert.equal(out.gamepad[0].seq, 2);
  assert.equal(pads.poll([pad({ buttons })], 370).gamepad.length, 1, 'kept alive');
  // Another pad in the same slot is a new controller.
  out = pads.poll([pad({ id: 'Other' })], 380);
  assert.deepEqual(out.control.map((m) => m.type), ['gamepad-disconnected', 'gamepad-connected']);
  out = pads.poll([null], 400);
  assert.deepEqual(out.control, [{ v: 2, type: 'gamepad-disconnected', controllerId: 0 }]);
  pads.poll([pad({ index: 3 })], 500);
  assert.deepEqual(pads.release(), [{ v: 2, type: 'gamepad-disconnected', controllerId: 3 }]);
  assert.deepEqual(pads.release(), []);
});

test('the owner’s panel offers one Invite button and no Experimental label', () => {
  const { body } = render(GuestPanel);
  assert.match(body, />Invite</);
  assert.doesNotMatch(body, /experiment/i);
});

test('the guest page asks for a name with a link, and says when the link is cut short', () => {
  location.hash = `#${TOKEN}`;
  let { body } = render(Guest);
  assert.match(body, /Your name/);
  assert.match(body, />Join</);
  assert.match(body, /id="guest-video"/);
  location.hash = `#${TOKEN.slice(4)}`;
  ({ body } = render(Guest));
  assert.match(body, /This invite link is incomplete/);
  assert.doesNotMatch(body, /Your name/);
});

test('the guest build is its own: base /guest/, its own entry, nothing of the console imported', () => {
  const config = readFileSync(new URL('./vite.guest.config.ts', import.meta.url), 'utf8');
  assert.match(config, /base: '\/guest\/'/);
  assert.match(config, /dist\/guest/);
  const html = readFileSync(new URL('./guest/index.html', import.meta.url), 'utf8');
  assert.match(html, /src\/guest\/main\.ts/);
  const page = readFileSync(new URL('./src/guest/Guest.svelte', import.meta.url), 'utf8');
  assert.doesNotMatch(page, /lib\/api|lib\/session|router/);
  const scripts = JSON.parse(readFileSync(new URL('./package.json', import.meta.url), 'utf8')).scripts;
  assert.match(scripts.build, /vite build && vite build --config vite\.guest\.config\.ts/);
});
