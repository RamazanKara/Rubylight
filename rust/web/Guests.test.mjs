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
const { default: JoinCard, PERMISSION_TEXT } = await server.ssrLoadModule('/src/guest/GuestJoinCard.svelte');
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
  // The link goes to the guest page's own port, never the console's.
  assert.equal(guests.linkAddress(page('localhost:47990'), ['192.168.4.69'], 47991), '192.168.4.69:47991');
  assert.equal(guests.linkAddress(page('127.0.0.1:47990'), ['192.168.4.69'], 47991), '192.168.4.69:47991');
  assert.equal(guests.linkAddress(page('[::1]:47990'), ['192.168.4.69'], 47991), '192.168.4.69:47991');
  assert.equal(guests.linkAddress(page('pc.local:47990'), ['192.168.4.69'], 47991), 'pc.local:47991');
  assert.equal(guests.linkAddress(page('localhost:47990'), [], 47991), 'localhost:47991');
  // An address the owner typed without a port gets the guest port; one with a port keeps it.
  assert.equal(guests.withPort('home.example.org', 47991), 'home.example.org:47991');
  assert.equal(guests.withPort(' home.example.org:443 ', 47991), 'home.example.org:443');
  assert.equal(guests.withPort('[2001:db8::1]', 47991), '[2001:db8::1]:47991');
  assert.equal(guests.withPort('[2001:db8::1]:8443', 47991), '[2001:db8::1]:8443');
  for (const good of ['192.168.4.69:47990', 'home.example.org', '[2001:db8::1]:47990']) assert.ok(guests.validAddress(good), good);
  for (const bad of ['', 'a b', 'host/path', 'https://host', 'host:port', 'host#x']) assert.ok(!guests.validAddress(bad), bad);
  assert.equal(guests.addressProblem('192.168.4.69:47990'), undefined);
  assert.equal(guests.addressProblem('host/path'), 'A host name or IP address, with an optional port');
  for (const local of ['localhost:47990', '127.0.0.1', '[::1]:47990', 'pc.localhost'])
    assert.equal(guests.addressProblem(local), 'Only this PC opens this address', local);
});

test('expiry reads in minutes, hours or days', () => {
  assert.equal(guests.expiresIn(1000 + 59 * 60, 1000), 'in 59 min');
  assert.equal(guests.expiresIn(1000 + 10, 1000), 'in 1 min');
  assert.equal(guests.expiresIn(1000 + 23 * 3600, 1000), 'in 23 h');
  assert.equal(guests.expiresIn(1000 + 6 * 86_400, 1000), 'in 6 days');
  assert.equal(guests.expiresIn(1000, 1000), 'expired');
  assert.equal(guests.expiresIn(900, 1000), 'expired');
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
  assert.deepEqual(guest.ending({}), { code: 'connection-lost', message: 'The connection to the PC was lost. Check your network and join again.', wait: false, final: false });
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

function joining(probe = async () => 200) {
  let socket;
  const endings = [];
  const stream = guest.GuestStream.join(TOKEN, 'Sam', {}, () => assert.fail('no stream'), 'wss://pc/guest/signal', (url, protocols) => (socket = new FakeSocket(url, protocols)), probe);
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

test('a refused socket, a revoked link and a lost connection each say so once', async () => {
  let probes = 0;
  let run = joining(async () => (probes++, 200));
  run.socket.onerror?.({});
  run.socket.onclose?.({});
  await new Promise((resolve) => setTimeout(resolve));
  assert.deepEqual(run.endings.map((e) => e.code), ['refused']);
  assert.equal(probes, 1);
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

// Rubylight 7.0: the join card on its dark stage and the stream's bar.

test('the join card says what the host said, and keeps a browser’s own error under Details', () => {
  const card = (props) => render(JoinCard, { props: { name: 'Sam', onjoin() {}, onleave() {}, ...props } }).body;
  let body = card({ phase: 'name' });
  assert.match(body, /Join the stream/);
  assert.match(body, /The host sees the name you pick/);
  assert.match(body, /maxlength="32"/);
  body = card({ phase: 'joining' });
  assert.match(body, /role="status"[^>]*>Joining…/);
  // Waiting: the host's own sentence, or the page's when the host sent none.
  body = card({ phase: 'waiting', waiting: 'The host has not started a game yet.' });
  assert.match(body, /Waiting for the host/);
  assert.match(body, /The host has not started a game yet\./);
  assert.match(body, />Leave</);
  assert.doesNotMatch(body, /View only|Controller/, 'the permission is unknown before the first offer');
  body = card({ phase: 'waiting', permission: 'controller' });
  assert.match(body, /This page joins when the stream starts\./);
  assert.match(body, />Controller</);
  assert.deepEqual(Object.keys(PERMISSION_TEXT), ['view', 'controller', 'controller-keyboard-mouse']);
  // The three endings, and who may join again.
  body = card({ phase: 'ended', ended: { code: 'left', message: 'You left the stream.', final: false } });
  assert.match(body, /You left/);
  assert.match(body, />Join again</);
  body = card({ phase: 'ended', ended: { code: 'host-ended', message: 'The host stopped the stream.', final: false } });
  assert.match(body, /The stream stopped/);
  assert.match(body, /role="alert"[^>]*>The host stopped the stream\./);
  body = card({ phase: 'ended', ended: { code: 'invalid-link', message: 'This invite link does not work any more.', final: true } });
  assert.match(body, /This link no longer works/);
  assert.doesNotMatch(body, /Join again/);
  // A browser's WebRTC error is no sentence for a guest: it folds under Details.
  body = card({ phase: 'ended', ended: { code: 'browser', message: 'This browser could not play the stream: InvalidAccessError: Failed to set remote answer sdp', final: false } });
  assert.match(body, /role="alert"[^>]*>This browser could not play the stream\. Try another browser, or update this one\./);
  assert.match(body, /<details[^>]*><summary[^>]*>Details<\/summary>[\s\S]*InvalidAccessError: Failed to set remote answer sdp/);
  assert.doesNotMatch(body.slice(0, body.indexOf('<details')), /InvalidAccessError/);
});

test('the stream’s bar hides itself, leaves a mouse-and-keyboard guest’s pointer alone, and the page lets go on leaving', () => {
  const page = readFileSync(new URL('./src/guest/Guest.svelte', import.meta.url), 'utf8');
  // The page is a dark stage in either theme, and the bar a toolbar beside the picture, not in it.
  assert.match(page, /<main class="media-plane"/);
  const surface = page.slice(page.indexOf('class="surface"'), page.indexOf('</video>'));
  assert.doesNotMatch(surface, /class="bar"/);
  assert.match(page, /class="bar"\s+class:shown=\{barShown \|\| paused\}\s+role="toolbar"\s+aria-label="Stream controls"/);
  for (const control of ["{fullscreen ? 'Exit full screen' : 'Full screen'}", "{muted ? 'Unmute' : 'Mute'}", 'onclick={leave}>Leave<']) assert.ok(page.includes(control), control);
  // Four seconds without a reach for it; held while the pointer or the focus is on it.
  assert.match(page, /const BAR_MS = 4000;/);
  assert.match(page, /barTimer = setTimeout\(\(\) => \(barShown = barHeld\), BAR_MS\);/);
  assert.match(page, /onfocusin=\{\(\) => hold\(true\)\}/);
  assert.match(page, /onpointerenter=\{\(\) => hold\(true\)\}/);
  // A guest whose mouse plays the game wakes the bar only at the top edge; a finger always does.
  assert.match(page, /if \(!typing \|\| event\.pointerType !== 'mouse' \|\| event\.clientY - \(player\?\.getBoundingClientRect\(\)\.top \?\? 0\) < BAR_ZONE\) wake\(\);/);
  // Leaving the page tells the host and lets go of the tracks.
  assert.match(page, /onpagehide=\{\(\) => \{\s+stream\?\.leave\(\);\s+stopped\(\);\s+\}\}/);
  // The bar's motion stops with reduced motion.
  assert.match(page, /@media \(prefers-reduced-motion: reduce\)/);
  // The join card shares the page's build: nothing of the console's API.
  assert.doesNotMatch(readFileSync(new URL('./src/guest/GuestJoinCard.svelte', import.meta.url), 'utf8'), /lib\/api|lib\/session|router/);
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

test('a waiting page whose link expired says the link does not work, not that it was refused', async () => {
  // The guest port answers 404 once no link works: the socket fails, and so does the page's own address.
  let run = joining(async () => 404);
  run.socket.onclose?.({});
  await new Promise((resolve) => setTimeout(resolve));
  assert.deepEqual(run.endings.map((e) => [e.code, e.final]), [['invalid-link', true]]);
  assert.match(run.endings[0].message, /does not work/);
  // A host that cannot be reached at all is still a refusal.
  run = joining(async () => {
    throw new TypeError('Failed to fetch');
  });
  run.socket.onerror?.({});
  await new Promise((resolve) => setTimeout(resolve));
  assert.deepEqual(run.endings.map((e) => [e.code, e.final]), [['refused', false]]);
});

// "On the internet" in the invite dialog: the guest ports the router forwarded, the address the
// link takes, and what works instead when nothing reaches the PC.
const { default: InternetReach } = await server.ssrLoadModule('/src/components/stream/InternetReach.svelte');

const reachFixture = (over = {}) => ({
  checking: false,
  runs: true,
  method: 'UPnP',
  error: null,
  ports: [
    { protocol: 'TCP', port: 47991, external: 47991, reason: null },
    ...[8002, 8003, 8004, 8005].map((port) => ({ protocol: 'UDP', port, external: port, reason: null })),
  ],
  publicIpv4: '203.0.113.5',
  routerIpv4: '203.0.113.5',
  ipv6: null,
  address: '203.0.113.5:47991',
  notes: [],
  internetPlay: false,
  ipv6Listening: false,
  guestJoin: 'ask',
  ...over,
});

test('an internet invite names the guest ports and takes the address the host found', () => {
  assert.equal(guests.guestPorts(47991, [8005, 8002, 8003, 8004]), 'TCP 47991, UDP 8002-8005');
  assert.equal(guests.guestPorts(47991, [8002, 8009]), 'TCP 47991, UDP 8002, UDP 8009');
  assert.equal(guests.guestPorts(47991, []), 'TCP 47991');
  assert.deepEqual(guests.AUDIENCES.map((a) => a.value), ['local', 'internet']);
  // The local choice keeps the console's address; the internet one takes the host's, or none.
  assert.equal(guests.suggestedAddress('local', '192.168.4.69:47991', reachFixture()), '192.168.4.69:47991');
  assert.equal(guests.suggestedAddress('internet', '192.168.4.69:47991', reachFixture()), '203.0.113.5:47991');
  assert.equal(guests.suggestedAddress('internet', '192.168.4.69:47991', reachFixture({ address: null })), '');
  assert.equal(guests.suggestedAddress('internet', '192.168.4.69:47991', null), '');
  assert.deepEqual(
    guests.portLines([
      { protocol: 'TCP', port: 47991, external: 47991, reason: null },
      { protocol: 'UDP', port: 8002, external: 18002, reason: null },
      { protocol: 'UDP', port: 8003, external: null, reason: 'the router is out of mappings' },
    ]),
    [
      { label: 'TCP 47991', ok: true, detail: 'forwarded' },
      { label: 'UDP 8002', ok: true, detail: "forwarded as 18002, which guests can't use" },
      { label: 'UDP 8003', ok: false, detail: 'the router is out of mappings' },
    ],
  );
});

test('the dialog says whether the internet reaches the PC before the link is sent', () => {
  assert.equal(guests.reachVerdict(null).tone, 'neutral');
  assert.equal(guests.reachVerdict(reachFixture({ checking: true })).tone, 'neutral');
  assert.deepEqual(guests.reachVerdict(reachFixture()), { tone: 'ok', text: 'Guests on the internet reach this PC at 203.0.113.5:47991.' });
  assert.match(guests.reachVerdict(reachFixture({ address: '[2001:db8::7]:47991' })).text, /over IPv6 .* They need IPv6 too/);
  assert.equal(guests.reachVerdict(reachFixture({ address: null })).tone, 'warn');
  assert.match(guests.reachVerdict(reachFixture({ runs: false, address: null })).text, /listens on this PC only/);
  // Nothing reaches it: say plainly what works instead.
  const instead = guests.reachAlternatives(reachFixture({ address: null }), 'TCP 47991, UDP 8002-8005');
  assert.match(instead[0], /Tailscale or another VPN/);
  assert.match(instead[1], /IPv6, if you both have it: set IP versions to IPv4 and IPv6/);
  assert.match(instead[2], /forward TCP 47991, UDP 8002-8005 on your router by hand/);
  assert.equal(instead.length, 3);
  assert.match(
    guests.reachAlternatives(reachFixture({ address: null, ipv6Listening: true }), 'TCP 47991')[1],
    /allow TCP 47991 in to this PC in your router's IPv6 firewall/,
  );
  const nightjar = guests.reachAlternatives(reachFixture({ address: null, internetPlay: true }), '')[3];
  assert.match(nightjar, /Nightjar .* Play over the internet/);
  // Ask the player or Let in without asking already let a guest in: no setting to name.
  assert.doesNotMatch(nightjar, /When another device joins/);
  assert.match(
    guests.reachAlternatives(reachFixture({ address: null, internetPlay: true, guestJoin: 'off' }), '')[3],
    /When another device joins the running game set to Ask the player or Let in without asking/,
  );
});

test('the reach panel shows each port, the public address and the carrier-grade NAT note', () => {
  let { body } = render(InternetReach, { props: { reach: reachFixture(), ports: 'TCP 47991, UDP 8002-8005' } });
  assert.match(body, /Reachable/);
  assert.match(body, /203\.0\.113\.5:47991/);
  assert.match(body, /UDP 8005/);
  assert.match(body, /forwards only TCP 47991, UDP 8002-8005 on your router/);
  assert.match(body, /also with automatic port forwarding off/);
  assert.doesNotMatch(body, /Tailscale/);
  assert.doesNotMatch(body, /experiment/i);

  const cgnat = reachFixture({
    address: null,
    publicIpv4: null,
    routerIpv4: '100.72.0.1',
    notes: ["The router's own internet address (100.72.0.1) is not public: your internet provider shares one address among its customers (carrier-grade NAT), so nothing on the internet can open a port on it."],
  });
  ({ body } = render(InternetReach, { props: { reach: cgnat, ports: 'TCP 47991, UDP 8002-8005' } }));
  assert.match(body, /Not reachable/);
  assert.match(body, /100\.72\.0\.1 \(not public\)/);
  assert.match(body, /carrier-grade NAT/);
  assert.match(body, /Tailscale or another VPN/);

  const refused = reachFixture({
    address: null,
    method: null,
    error: 'the router answers none of UPnP, PCP and NAT-PMP: turn UPnP on in the router, or forward the ports by hand (no UPnP router found)',
    ports: [{ protocol: 'TCP', port: 47991, external: null, reason: 'the router refused the mapping' }],
  });
  ({ body } = render(InternetReach, { props: { reach: refused, ports: 'TCP 47991' } }));
  assert.match(body, /Not forwarded/);
  assert.match(body, /answers none of UPnP, PCP and NAT-PMP/);

  ({ body } = render(InternetReach, { props: { reach: null, ports: 'TCP 47991' } }));
  assert.match(body, /Asking the router to forward the guest ports/);
});
