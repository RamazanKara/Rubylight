import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

// The play page (docs/webrtc.md, "Play in a TV browser"): its sign-in, saved device and navigation.
globalThis.location = new URL('https://192.168.4.69:47991/play/');
globalThis.window = new EventTarget();
const server = await createTestServer();
const play = await server.ssrLoadModule('/src/lib/webrtc/play.ts');
const { default: Play } = await server.ssrLoadModule('/src/play/Play.svelte');
const { render } = await server.ssrLoadModule('svelte/server');

const SECRET = Array.from({ length: 32 }, (_, i) => i.toString(16).padStart(2, '0')).join('');
const NONCE = Array.from({ length: 32 }, (_, i) => (0xa0 + i).toString(16)).join('');
const ID = '00112233445566778899aabbccddeeff';

function storage() {
  const items = new Map();
  return {
    items,
    getItem: (key) => items.get(key) ?? null,
    setItem: (key, value) => items.set(key, String(value)),
    removeItem: (key) => items.delete(key),
  };
}

function pad(pressed = [], axes = [0, 0, 0, 0]) {
  const buttons = Array.from({ length: 17 }, (_, i) => ({ pressed: pressed.includes(i), value: pressed.includes(i) ? 1 : 0 }));
  return { index: 0, id: 'Xbox Wireless Controller (STANDARD GAMEPAD)', mapping: 'standard', buttons, axes };
}

test('the proof is the TV protocol’s HMAC of the nonce, and the socket is the page’s own host', async () => {
  // The vector of rust/host/src/webrtc/clients.rs.
  assert.equal(await play.proof(SECRET, NONCE), '6cc53d0e9647fd7c04a198c1c2f1bbb425ecf7957567b61208033dfd029c9a63');
  await assert.rejects(play.proof(SECRET, NONCE, null), /https:\/\//);
  assert.equal(play.playSignalUrl(location), 'wss://192.168.4.69:47991/play/signal');
  assert.equal(play.PLAY_PROTOCOL, 'rubylight.play.v2');
});

test('a PIN is four digits from the random source', () => {
  assert.equal(play.newPin((a) => (a[0] = 7)), '0007');
  assert.equal(play.newPin((a) => (a[0] = 4_294_967_295)), '7295');
  assert.match(play.newPin(), /^\d{4}$/);
});

test('the device is kept in site data, and only a well-formed one is used', () => {
  const site = storage();
  assert.equal(play.loadCredentials(site), null);
  play.saveCredentials({ clientId: ID, secret: SECRET }, site);
  assert.deepEqual(play.loadCredentials(site), { clientId: ID, secret: SECRET });
  for (const bad of ['', 'null', '{', JSON.stringify({ clientId: ID }), JSON.stringify({ clientId: ID.toUpperCase(), secret: SECRET }), JSON.stringify({ clientId: ID, secret: SECRET.slice(2) })]) {
    assert.equal(play.parseCredentials(bad), null, bad);
  }
  play.forgetCredentials(site);
  assert.equal(site.items.size, 0);
  // A browser that keeps nothing says so instead of pairing silently on every visit.
  const blocked = { setItem: () => { throw new Error('SecurityError'); }, getItem: () => { throw new Error('SecurityError'); } };
  assert.throws(() => play.saveCredentials({ clientId: ID, secret: SECRET }, blocked), /does not keep site data/);
  assert.equal(play.loadCredentials(blocked), null);
});

test('the Devices page names the browser by its device', () => {
  const name = (agent) => play.deviceName(agent);
  assert.equal(name('Mozilla/5.0 (Windows NT 10.0; Win64; x64; Xbox; Xbox Series X) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36 Edg/130.0.0.0'), 'Xbox');
  assert.equal(name('Mozilla/5.0 (Web0S; Linux/SmartTV) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/108.0.0.0 Safari/537.36'), 'LG TV');
  assert.equal(name('Mozilla/5.0 (SMART-TV; Linux; Tizen 7.0) AppleWebKit/537.36 (KHTML, like Gecko) SamsungBrowser/5.0 Chrome/94.0 TV Safari/537.36'), 'Samsung TV');
  assert.equal(name('Mozilla/5.0 (X11; Linux) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0 Safari/537.36 Tesla/2024.1'), 'Car');
  assert.equal(name('Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36 Edg/131.0.0.0'), 'Edge on Windows');
  assert.equal(name('Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36'), 'Chrome on Windows');
  assert.equal(name('Mozilla/5.0 (X11; Ubuntu; Linux x86_64; rv:133.0) Gecko/20100101 Firefox/133.0'), 'Firefox on Linux');
  assert.equal(name(''), 'Browser');
});

test('the arrows move along a grid’s rows and columns, and nowhere past its edge', () => {
  // Three tiles in a row, then one below the first.
  const box = (left, top) => ({ left, top, width: 100, height: 140 });
  const grid = [box(0, 0), box(120, 0), box(240, 0), box(0, 160)];
  assert.equal(play.nearest(grid[0], grid, 'right'), 1);
  assert.equal(play.nearest(grid[1], grid, 'right'), 2);
  assert.equal(play.nearest(grid[2], grid, 'right'), -1);
  assert.equal(play.nearest(grid[0], grid, 'down'), 3);
  // Down from the last column goes to the row below, its nearest tile.
  assert.equal(play.nearest(grid[2], grid, 'down'), 3);
  assert.equal(play.nearest(grid[3], grid, 'up'), 0);
  assert.equal(play.nearest(grid[3], grid, 'left'), -1);
  assert.equal(play.nearest(grid[0], grid, 'up'), -1);
});

test('arrows, Escape and a TV remote’s Back are the page’s keys', () => {
  const key = (key, keyCode = 0) => play.keyAction({ key, keyCode });
  assert.deepEqual(['ArrowUp', 'ArrowDown', 'ArrowLeft', 'ArrowRight'].map((k) => key(k)), ['up', 'down', 'left', 'right']);
  for (const back of ['Escape', 'GoBack', 'BrowserBack', 'XF86Back']) assert.equal(key(back), 'back', back);
  assert.equal(key('Unidentified', 461), 'back');
  assert.equal(key('Unidentified', 10009), 'back');
  assert.equal(key('Enter'), null);
  assert.equal(key('a'), null);
});

test('a pad press acts once, a held direction repeats, and Select held a second opens the menu', () => {
  const nav = new play.PadNavigator();
  assert.deepEqual(nav.poll([pad([0])], 0), ['select']);
  assert.deepEqual(nav.poll([pad([0])], 2000), []);
  assert.deepEqual(nav.poll([pad()], 2016), []);
  assert.deepEqual(nav.poll([null, pad([1])], 2032), ['back']);
  // The D-pad and the left stick move; a held one repeats after 400 ms, then every 150 ms.
  assert.deepEqual(nav.poll([pad([15])], 3000), ['right']);
  assert.deepEqual(nav.poll([pad([15])], 3399), []);
  assert.deepEqual(nav.poll([pad([15])], 3400), ['right']);
  assert.deepEqual(nav.poll([pad([15])], 3500), []);
  assert.deepEqual(nav.poll([pad([15])], 3550), ['right']);
  assert.deepEqual(nav.poll([pad([], [0, 0.9, 0, 0])], 4000), ['down']);
  assert.deepEqual(nav.poll([pad([], [0, 0.5, 0, 0])], 4016), []);
  assert.deepEqual(nav.poll([pad([], [-0.9, 0, 0, 0])], 4032), ['left']);
  // Select (Back, View) opens the menu once per hold, after a second.
  assert.deepEqual(nav.poll([pad([8])], 5000), []);
  assert.deepEqual(nav.poll([pad([8])], 5999), []);
  assert.deepEqual(nav.poll([pad([8])], 6000), ['menu']);
  assert.deepEqual(nav.poll([pad([8])], 9000), []);
  assert.deepEqual(nav.poll([pad()], 9016), []);
  assert.deepEqual(nav.poll([pad([8])], 9032), []);
  assert.deepEqual(nav.poll([pad([8])], 10_032), ['menu']);
});

test('a key or button held a second counts once, and letting go starts over', () => {
  const hold = new play.Hold();
  assert.equal(hold.update(true, 0), false);
  assert.equal(hold.update(true, 999), false);
  assert.equal(hold.update(true, 1000), true);
  assert.equal(hold.update(true, 5000), false);
  assert.equal(hold.update(false, 5016), false);
  assert.equal(hold.update(true, 5032), false);
  assert.equal(hold.update(true, 6032), true);
  // The menu sends the host this browser's pads let go.
  const held = pad([0, 12], [0.5, -1, 0, 0]);
  const released = play.released(held);
  assert.ok(released.buttons.every((b) => !b.pressed && b.value === 0));
  assert.deepEqual(released.axes, [0, 0, 0, 0]);
  assert.equal(released.id, held.id);
});

test('a key repeat of Back never closes the menu it just opened', () => {
  const page = readFileSync(new URL('./src/play/Play.svelte', import.meta.url), 'utf8');
  const windowKey = page.slice(page.indexOf('function windowKey'), page.indexOf('function key('));
  assert.ok(windowKey.indexOf("action === 'back' && event.repeat") >= 0 &&
    windowKey.indexOf("action === 'back' && event.repeat") < windowKey.indexOf('act(action)'), windowKey);
  // The focus moves to the menu and the picture never sees the key let go.
  const openMenu = page.slice(page.indexOf('function openMenu'), page.indexOf('function closeMenu'));
  assert.match(openMenu, /backKey = false/);
});

test('the page opens on Connecting with its picture ready and no Experimental label', () => {
  const { body } = render(Play);
  assert.match(body, /Connecting…/);
  assert.match(body, /id="play-video"/);
  assert.doesNotMatch(body, /experiment/i);
  const page = readFileSync(new URL('./src/play/Play.svelte', import.meta.url), 'utf8');
  for (const text of ['Pair with your PC', 'Disconnect', 'Quit game', 'Hold Back or Select']) assert.ok(page.includes(text), text);
});

test('the play build is its own: base /play/, its own entry, nothing of the console imported', () => {
  const config = readFileSync(new URL('./vite.play.config.ts', import.meta.url), 'utf8');
  assert.match(config, /base: '\/play\/'/);
  assert.match(config, /dist\/play/);
  const html = readFileSync(new URL('./play/index.html', import.meta.url), 'utf8');
  assert.match(html, /src\/play\/main\.ts/);
  for (const file of ['./src/play/Play.svelte', './src/lib/webrtc/play.ts']) {
    const source = readFileSync(new URL(file, import.meta.url), 'utf8');
    assert.doesNotMatch(source, /lib\/api|lib\/session|router|csrf/i, file);
  }
  const scripts = JSON.parse(readFileSync(new URL('./package.json', import.meta.url), 'utf8')).scripts;
  assert.match(scripts.build, /vite build --config vite\.play\.config\.ts/);
});
