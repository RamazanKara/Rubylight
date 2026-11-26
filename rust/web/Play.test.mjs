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
// Rubylight 7.0: the play page's parts.
const source = await server.ssrLoadModule('/src/play/play-input-source.ts');
const statsLib = await server.ssrLoadModule('/src/lib/webrtc/stats.ts');
const part = async (name) => (await server.ssrLoadModule(`/src/play/${name}.svelte`)).default;
const [Dock, GlyphBar, Library, PairCard, StatsCapsule] = await Promise.all(['PlayDock', 'PlayGlyphBar', 'PlayLibrary', 'PlayPairCard', 'PlayStatsCapsule'].map(part));
const PLAY_FILES = ['Play', 'PlayDock', 'PlayErrorCard', 'PlayGlyphBar', 'PlayLibrary', 'PlayPairCard', 'PlayStatsCapsule'].map((name) => `./src/play/${name}.svelte`).concat('./src/play/play-input-source.ts');

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
  const sources = PLAY_FILES.map((file) => readFileSync(new URL(file, import.meta.url), 'utf8')).join('\n');
  for (const text of ['Pair with your PC', 'Back to game', 'Disconnect', 'Quit game', 'Can’t play right now']) assert.ok(sources.includes(text), text);
});

test('the play build is its own: base /play/, its own entry, nothing of the console imported', () => {
  const config = readFileSync(new URL('./vite.play.config.ts', import.meta.url), 'utf8');
  assert.match(config, /base: '\/play\/'/);
  assert.match(config, /dist\/play/);
  const html = readFileSync(new URL('./play/index.html', import.meta.url), 'utf8');
  assert.match(html, /src\/play\/main\.ts/);
  for (const file of [...PLAY_FILES, './src/lib/webrtc/play.ts', './src/lib/webrtc/stats.ts']) {
    const source = readFileSync(new URL(file, import.meta.url), 'utf8');
    assert.doesNotMatch(source, /lib\/api|lib\/session|router|csrf/i, file);
  }
  const scripts = JSON.parse(readFileSync(new URL('./package.json', import.meta.url), 'utf8')).scripts;
  assert.match(scripts.build, /vite build --config vite\.play\.config\.ts/);
});

// Rubylight 7.0: the play dock, the stats capsule, the glyph bar and the screens around them.

test('a pad is drawn as its family, and the page follows the input last used', () => {
  assert.equal(source.padFamily('Xbox Wireless Controller (STANDARD GAMEPAD Vendor: 045e Product: 0b13)'), 'xbox');
  assert.equal(source.padFamily('DualSense Wireless Controller (STANDARD GAMEPAD Vendor: 054c Product: 0ce6)'), 'playstation');
  assert.equal(source.padFamily('Wireless Controller (STANDARD GAMEPAD Vendor: 054c Product: 09cc)'), 'playstation');
  assert.equal(source.padFamily('Pro Controller (STANDARD GAMEPAD Vendor: 057e Product: 2009)'), 'nintendo');
  assert.equal(source.padFamily('8BitDo Pro 2'), 'xbox', 'an unknown pad draws as an Xbox one');
  // A pad at rest changes nothing; a press or a moved stick makes it the input in hand.
  const sony = { ...pad(), id: 'DualSense Wireless Controller' };
  assert.equal(source.activePad([null, sony]), null);
  assert.equal(source.activePad([pad([0])]), 'xbox');
  assert.equal(source.activePad([{ ...sony, axes: [0, 0, 0.7, 0] }]), 'playstation');
  assert.equal(source.activePad([{ ...sony, axes: [0.2, 0, 0, 0] }]), null, 'stick drift is no input');
  // Keys: a keyboard, unless the browser is a TV's or the key is a remote's own.
  const tv = 'Mozilla/5.0 (Web0S; Linux/SmartTV) AppleWebKit/537.36 Chrome/108.0.0.0 Safari/537.36';
  const desktop = 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/131.0.0.0 Safari/537.36';
  assert.equal(source.keySource({ key: 'Enter' }, desktop), 'keyboard');
  assert.equal(source.keySource({ key: 'Enter' }, tv), 'remote');
  assert.equal(source.keySource({ key: 'GoBack' }, desktop), 'remote');
  assert.equal(source.keySource({ key: 'Unidentified', keyCode: 10009 }, desktop), 'remote');
  assert.equal(source.pointerSource('mouse'), 'mouse');
  assert.equal(source.pointerSource('touch'), 'touch');
  assert.equal(source.pointerSource('pen'), 'touch');
  // Before anything was pressed: a remote on a TV, a pad on an Xbox, a finger on a phone.
  assert.equal(source.firstSource(tv, false), 'remote');
  assert.equal(source.firstSource('Mozilla/5.0 (Windows NT 10.0; Win64; x64; Xbox; Xbox Series X) Edg/130.0', false), 'xbox');
  assert.equal(source.firstSource(desktop, true), 'touch');
  assert.equal(source.firstSource(desktop, false), 'mouse');
});

test('the hints name each input’s own buttons, and touch and mouse get none', () => {
  const glyphs = (from, context) => source.hints(from, context).map((hint) => source.hintText(hint));
  for (const context of ['pair-failed', 'library', 'stream', 'dock', 'error']) {
    assert.deepEqual(source.hints('touch', context), [], `touch, ${context}`);
    assert.deepEqual(source.hints('mouse', context), [], `mouse, ${context}`);
  }
  assert.deepEqual(glyphs('xbox', 'library'), ['D-pad: Move', 'A: Play']);
  assert.deepEqual(glyphs('xbox', 'stream'), ['Hold View: Menu']);
  assert.deepEqual(glyphs('xbox', 'dock'), ['D-pad: Move', 'A: Choose', 'B: Back to game']);
  // The page reads a pad by position: Sony's cross and circle, and Nintendo's letters the other way round.
  assert.deepEqual(glyphs('playstation', 'dock'), ['D-pad: Move', '×: Choose', '○: Back to game']);
  assert.deepEqual(glyphs('playstation', 'stream'), ['Hold Create: Menu']);
  assert.deepEqual(glyphs('nintendo', 'dock'), ['D-pad: Move', 'B: Choose', 'A: Back to game']);
  assert.deepEqual(glyphs('keyboard', 'library'), ['Arrows: Move', 'Enter: Play']);
  assert.deepEqual(glyphs('keyboard', 'stream'), ['Hold Esc: Menu']);
  assert.deepEqual(glyphs('remote', 'library'), ['Arrows: Move', 'OK: Play']);
  assert.deepEqual(glyphs('remote', 'stream'), ['Hold Back: Menu']);
  assert.deepEqual(glyphs('remote', 'error'), ['OK: Try again']);
  assert.deepEqual(glyphs('keyboard', 'pair-failed'), ['Enter: New PIN']);
  // The same in the bar: nothing at all for a pointer, round face buttons for a pad.
  assert.equal(render(GlyphBar, { props: { source: 'mouse', context: 'library' } }).body.includes('<ul'), false);
  assert.equal(render(GlyphBar, { props: { source: 'touch', context: 'stream', floating: true } }).body.includes('<ul'), false);
  const bar = render(GlyphBar, { props: { source: 'playstation', context: 'dock' } }).body;
  assert.match(bar, /<ul class="glyphs[^"]*" aria-label="Controls">/);
  assert.match(bar, /<kbd[^>]*class="[^"]*round[^"]*"[^>]*>×<\/kbd>/);
  assert.match(bar, /×: Choose/);
  assert.match(render(GlyphBar, { props: { source: 'keyboard', context: 'stream', floating: true } }).body, /class="glyphs[^"]*floating/);
});

test('the capsule’s numbers come from two looks at the browser’s own count', () => {
  const report = (bytes, frames, rtt = 0.004) => new Map([
    ['v', { type: 'inbound-rtp', kind: 'video', codecId: 'c', bytesReceived: bytes, framesDecoded: frames, frameWidth: 1920, frameHeight: 1080 }],
    ['a', { type: 'inbound-rtp', kind: 'audio', bytesReceived: 9_999_999 }],
    ['c', { type: 'codec', id: 'c', mimeType: 'video/H265' }],
    ['p', { type: 'candidate-pair', state: 'succeeded', nominated: true, currentRoundTripTime: rtt }],
  ]);
  // The first look has no rate yet.
  const first = statsLib.readStats(report(1_000_000, 100), null, 10_000);
  assert.deepEqual(first.stats, { fps: null, mbps: null, rttMs: 4, codec: 'H265', width: 1920, height: 1080 });
  // One second later: 60 frames and 2.5 MB more are 60 fps and 20 Mbit/s.
  const second = statsLib.readStats(report(3_500_000, 160), first.look, 11_000);
  assert.equal(second.stats.fps, 60);
  assert.equal(second.stats.mbps, 20);
  // A new stream counts from zero: no rate across the restart.
  const restarted = statsLib.readStats(report(50_000, 3), second.look, 12_000);
  assert.equal(restarted.stats.fps, null);
  assert.equal(restarted.stats.mbps, null);
  // No video yet: nothing to show, and nothing kept.
  const none = statsLib.readStats(new Map(), second.look, 13_000);
  assert.equal(none.look, null);
  assert.equal(none.stats.rttMs, null);
  assert.equal(statsLib.statText(null), '–');
  assert.equal(statsLib.statText(59.94), '60');
  assert.equal(statsLib.statText(19.96, 1), '20.0');

  let body = render(StatsCapsule, { props: { stats: null, codec: 'HEVC · 1080p60' } }).body;
  assert.match(body, /aria-label="Stream statistics"/);
  assert.match(body, /aria-live="off"/);
  assert.match(body, /HEVC · 1080p60/);
  assert.equal((body.match(/–/g) ?? []).length, 3, 'FPS, Mbps and RTT wait for the browser');
  body = render(StatsCapsule, { props: { stats: second.stats, codec: 'HEVC · 1080p60' } }).body;
  for (const shown of ['>60<', '>20.0<', '>4<', 'HEVC 1080p']) assert.ok(body.includes(shown), shown);
  assert.match(readFileSync(new URL('./src/play/PlayStatsCapsule.svelte', import.meta.url), 'utf8'), /font-variant-numeric: tabular-nums/);
});

test('the dock holds every stream control, each one reachable by remote, and one danger control', () => {
  const props = { title: 'Portal 2', picture: 'HEVC · 1080p60', muted: false, stats: true, fullscreen: false, canFullscreen: true, source: 'xbox',
    onresume() {}, onfullscreen() {}, onmute() {}, onstats() {}, ondisconnect() {}, onquit() {} };
  let body = render(Dock, { props }).body;
  assert.match(body, /role="dialog" aria-modal="true" aria-label="Stream menu"/);
  assert.match(body, /Portal 2/);
  assert.match(body, /HEVC · 1080p60/);
  const controls = body.match(/<button class="control[^"]*"[^>]*>/g) ?? [];
  assert.equal(controls.length, 6);
  assert.ok(controls.every((control) => control.includes('data-nav')), 'a remote reaches every control');
  for (const label of ['Back to game', 'Full screen', 'Sound', 'Stats', 'Disconnect', 'Quit game']) assert.ok(body.includes(`>${label}<`), label);
  assert.equal((body.match(/class="control[^"]*danger/g) ?? []).length, 1);
  assert.match(body, /class="control[^"]*danger[^>]*>(?:(?!<\/button>)[\s\S])*Quit game/);
  // Full screen, Sound and Stats say their state, in words and to a screen reader.
  assert.equal((body.match(/aria-pressed="true"/g) ?? []).length, 2);
  assert.equal((body.match(/aria-pressed="false"/g) ?? []).length, 1, 'not in full screen yet');
  assert.match(body, /A: Choose/);
  body = render(Dock, { props: { ...props, muted: true, stats: false, fullscreen: true, canFullscreen: true, source: 'mouse' } }).body;
  assert.equal((body.match(/aria-pressed="false"/g) ?? []).length, 2);
  assert.equal((body.match(/aria-pressed="true"/g) ?? []).length, 1, 'full screen is on');
  assert.doesNotMatch(body, /aria-label="Controls"/, 'no hints for a mouse');
  // An iPhone has no full screen to offer.
  body = render(Dock, { props: { ...props, canFullscreen: false } }).body;
  assert.doesNotMatch(body, /Full screen/i);
  // Every control is at least 44 px tall.
  assert.match(readFileSync(new URL('./src/play/PlayDock.svelte', import.meta.url), 'utf8'), /min-height: max\(44px, [\d.]+em\)/);
});

test('nothing blurs over the picture, and the capsule is this browser’s choice', () => {
  const files = [...PLAY_FILES, './src/guest/Guest.svelte', './src/guest/GuestJoinCard.svelte', './src/styles/media-plane.css'];
  for (const file of files) {
    const text = readFileSync(new URL(file, import.meta.url), 'utf8').replace(/\/\*[\s\S]*?\*\/|<!--[\s\S]*?-->|^\s*(?:\/\/|\*).*$/gm, '');
    assert.doesNotMatch(text, /backdrop-filter/, file);
  }
  const page = readFileSync(new URL('./src/play/Play.svelte', import.meta.url), 'utf8');
  // The dock, the capsule, the hint and the handle only exist beside the picture, never inside its surface.
  const surface = page.slice(page.indexOf('class="surface"'), page.indexOf('</video>'));
  assert.doesNotMatch(surface, /PlayDock|PlayStatsCapsule|PlayGlyphBar|handle/);
  // Stats on or off is kept in site data and asked of the browser, once a second, only while it shows.
  assert.match(page, /const STATS_KEY = 'rubylight\.play\.stats'/);
  assert.match(page, /if \(phase !== 'stream' \|\| !statsOn\) return;/);
  assert.match(page, /localStorage\.setItem\(STATS_KEY/);
  // The handle is the pointer's way in; it never takes the focus from the game.
  assert.match(page, /<button class="handle"[^>]*tabindex="-1"[^>]*aria-label="Stream menu"[^>]*onclick=\{openMenu\}/);
  assert.match(page, /\{#if hint\}<PlayGlyphBar \{source\} context="stream" floating \/>\{\/if\}/);
});

test('the pairing card shows the PIN a digit to a cell, and a spent PIN offers a new one', () => {
  let body = render(PairCard, { props: { pin: '0427', name: 'LG TV', error: '', onretry() {} } }).body;
  assert.match(body, /Pair with your PC/);
  assert.match(body, /aria-label="PIN 0 4 2 7"/);
  assert.equal((body.match(/<span aria-hidden="true"[^>]*>\d<\/span>/g) ?? []).length, 4);
  assert.match(body, /Waiting for the PC…/);
  assert.match(body, /LG TV/);
  assert.doesNotMatch(body, /New PIN/);
  body = render(PairCard, { props: { pin: '0427', name: 'LG TV', error: 'The PIN ran out before the PC confirmed it.', onretry() {} } }).body;
  assert.match(body, /role="alert"[^>]*>The PIN ran out/);
  assert.match(body, /<button class="action[^"]*" data-nav[^>]*>New PIN/);
  assert.doesNotMatch(body, /Waiting for the PC/);
});

test('the library names the game in focus and gives every game a tile a remote reaches', () => {
  const apps = [
    { id: 'a', title: 'Portal 2', artworkAvailable: true, running: true },
    { id: 'b', title: 'Forza Horizon 5', artworkAvailable: false, running: false },
  ];
  let body = render(Library, { props: { apps, covers: { a: 'data:image/png;base64,AAAA' }, loading: false, onstart() {} } }).body;
  assert.equal((body.match(/<button class="tile[^"]*" data-nav/g) ?? []).length, 2);
  assert.match(body, /<h1[^>]*>Portal 2<\/h1>/);
  assert.match(body, /Running on the PC/);
  assert.match(body, /aria-label="Portal 2, running"/);
  assert.match(body, /<img src="data:image\/png;base64,AAAA" alt=""/);
  // A game without a cover shows its initials, the ones the console shows.
  assert.match(body, />FH</);
  body = render(Library, { props: { apps: [], covers: {}, loading: false, onstart() {} } }).body;
  assert.match(body, /No games yet/);
  assert.match(body, /Add them in the Rubylight console/);
  body = render(Library, { props: { apps: [], covers: {}, loading: true, onstart() {} } }).body;
  assert.match(body, /Loading games…/);
});
