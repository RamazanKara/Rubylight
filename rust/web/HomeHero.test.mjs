import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

globalThis.location = new URL('http://localhost/');
globalThis.window = new EventTarget();
const server = await createTestServer();
const { heroGame, lastPlayed, liveFigures, readyCopy, pairedCopy } = await server.ssrLoadModule('/src/components/overview/now-playing.ts');
const { knownDevices, newlyPaired } = await server.ssrLoadModule('/src/components/devices/paired.ts');
const { applyFix, fixes } = await server.ssrLoadModule('/src/lib/host-fixes.svelte.ts');
const { problems } = await server.ssrLoadModule('/src/lib/host-readiness.ts');
const { toasts } = await server.ssrLoadModule('/src/lib/feedback.svelte.ts');
const { session } = await server.ssrLoadModule('/src/lib/session.svelte.ts');

const source = (path) => readFileSync(new URL(`./src/${path}`, import.meta.url), 'utf8').replace(/\s+/g, ' ');
const apps = [
  { uuid: 'desktop', name: 'Desktop' },
  { uuid: 'portal', name: 'Portal 2', 'steam-id': '620', 'last-played': 1_700_000_000 },
  { uuid: 'hades', name: 'Hades II', 'playnite-id': 'h', 'last-played': 1_750_000_000 },
  { uuid: 'never', name: 'Never played', 'steam-id': '440' },
];

test('the hero shows the game that runs, else the one the stores last saw played', () => {
  assert.equal(lastPlayed(apps)?.name, 'Hades II');
  assert.equal(lastPlayed([apps[0], apps[3]]), null);
  assert.deepEqual(heroGame(apps, { appRunning: false }), { app: apps[2], name: 'Hades II', state: 'last' });
  // By uuid when the host names it: two apps may share a name.
  const running = { appRunning: true, paused: false, appName: 'Portal 2', app: { name: 'Portal 2', id: 2, uuid: 'portal' } };
  assert.deepEqual(heroGame(apps, running), { app: apps[1], name: 'Portal 2', state: 'running' });
  assert.equal(heroGame(apps, { ...running, paused: true }).state, 'paused');
  // An app no longer in the library still shows by its name.
  assert.deepEqual(heroGame(apps, { appRunning: true, paused: false, appName: 'Gone', app: { name: 'Gone', id: 9, uuid: null } }),
    { app: null, name: 'Gone', state: 'running' });
  assert.equal(heroGame([], { appRunning: false }), null);
});

test('a paired Home says where a game starts: the device, and this PC by name', () => {
  const plain = (text) => text.replace(/\u00a0/g, ' ');
  assert.equal(plain(readyCopy('Gaming PC')), 'Open Nightjar or Moonlight on your device and choose Gaming PC to play.');
  // The PC's name stays whole at a line break.
  assert.match(readyCopy('Gaming PC'), /Gaming\u00a0PC/);
  assert.equal(plain(readyCopy('  ')), 'Open Nightjar or Moonlight on your device and choose this PC to play.');
  assert.equal(plain(pairedCopy('Pixel 9', 'Gaming PC')), 'Pixel 9 is paired. Open Nightjar or Moonlight on it and choose Gaming PC to play.');
  const overview = source('pages/Overview.svelte');
  // The idle hero and the just-paired one both name the way in, from one helper.
  assert.match(overview, /if \(paired\) return readyCopy\(metadata\?\.host_name\);/);
  assert.match(overview, /welcomed \? pairedCopy\(welcomed\.name, metadata\?\.host_name\)/);
});

test('the live figures are the first running stream’s, measured or a dash', () => {
  const stream = (name, state, sample_frames = 240) => ({
    device_name: name, state, performance: { fps: 119.6, bitrate_mbps: 47.62, sample_frames,
      history: [{ bitrate_mbps: 40 }, { bitrate_mbps: 48 }] },
  });
  assert.equal(liveFigures([]), null);
  assert.equal(liveFigures([stream('TV', 'STOPPING')]), null);
  assert.deepEqual(liveFigures([stream('TV', 'STOPPING'), stream('Pixel 9', 'RUNNING'), stream('Deck', 'RUNNING')]),
    { device: 'Pixel 9', more: 1, fps: 120, mbps: 47.62, trend: [40, 48] });
  const quiet = liveFigures([stream('Pixel 9', 'RUNNING', 0)]);
  assert.equal(quiet.fps, null);
  assert.equal(quiet.mbps, null);
  // The bitrate line comes from the stream poll Home already runs every 2 s.
  const overview = source('pages/Overview.svelte');
  assert.match(overview, /poll\(refreshSessions, 2000\)/);
  assert.match(overview, /<Sparkline values=\{figures\.trend\}/);
});

test('a device that pairs while the panel is open is the new one, never one already there', () => {
  const pixel = { uuid: 'pixel', name: 'Pixel 9' };
  const tv = { uuid: 'tv', name: 'TV' };
  assert.equal(newlyPaired(null, [pixel]), null, 'the first list is what was there');
  const known = knownDevices([tv]);
  assert.equal(newlyPaired(known, [tv]), null);
  assert.equal(newlyPaired(known, [tv, pixel]), pixel);
  assert.equal(newlyPaired(known, []), null);
  const panel = source('components/devices/PairPanel.svelte');
  // Home has no device poll of its own: the panel polls as often as Devices does, which hands it its list.
  assert.match(panel, /if \(clients !== undefined\) return; return poll\(async \(\) => \{ own = \(await api\.clients\.list\(\)\)\.clients; \}, 5000\);/);
  assert.match(source('pages/Devices.svelte'), /<PairPanel \{clients\} \/>/);
  assert.match(source('pages/Devices.svelte'), /poll\(\(\) => Promise\.all\(\[load\(\), loadUsb\(\)\]\), 5000\)/);
  // The QR tile becomes the device's card, and the way back to the code.
  assert.match(panel, /<h3 id="pair-welcome">\{welcome\.name\} is ready<\/h3>/);
  assert.match(panel, /<Button onclick=\{\(\) => \(welcome = null\)\}>Pair another device<\/Button>/);
});

test('pairing leads an unpaired Home, the access choice waits for a second device, and pairing off keeps the code in place', () => {
  const panel = source('components/devices/PairPanel.svelte');
  assert.match(panel, /choosing = \$derived\(\(session\.metadata\?\.paired_devices \?\? 0\) > 0 && !pairingOff\)/);
  assert.match(panel, /\{#if choosing && !welcome\}/);
  assert.doesNotMatch(panel, /class="step"|Type the PIN it shows here|Open the app on your device/);
  assert.match(panel, /<QrPairing \{access\} lock=\{pairingOff \|\| qrOff \? unlock : undefined\}/);
  const qr = source('components/devices/QrPairing.svelte');
  // Locked, no code is asked for: a code made while pairing is off would expire unused.
  assert.match(qr, /if \(!inView \|\| !pageShown \|\| lock\) return;/);
  const overview = source('pages/Overview.svelte');
  assert.match(overview, /firstRun = \$derived\(Boolean\(metadata\) && !unavailable && !live && !top\?\.blocking && \(!paired \|\| welcomed !== null\)\)/);
  assert.match(overview, /<PairPanel bare onpaired=\{\(client\) => \(welcomed = client\)\} \/>/);
  // Launchers sync by themselves: Home has no Add games.
  assert.match(overview, /<PageHeader title="Home" \/>/);
  assert.doesNotMatch(overview, /Add games|Choose a game in your streaming app/);
});

/** Answers the console's requests in order; each answer is [path, body] or [path, body, status]. */
function serve(answers) {
  const sent = [];
  const realFetch = globalThis.fetch;
  globalThis.fetch = async (url, init) => {
    if (String(url).endsWith('/api/csrf-token')) return new Response('{"csrf_token":"t"}', { status: 200 });
    sent.push([init?.method ?? 'GET', String(url), init?.body ? JSON.parse(init.body) : undefined]);
    const [path, body, status = 200] = answers.shift() ?? ['', { error: 'unexpected' }, 500];
    assert.ok(String(url).includes(path), `${url} answered as ${path}`);
    return new Response(JSON.stringify(body), { status, headers: { 'Content-Type': 'application/json' } });
  };
  return { sent, restore: () => (globalThis.fetch = realFetch) };
}

const metadata = {
  paired_devices: 1, warnings: [],
  encoder_status: { state: 'ready', h264: true },
  virtual_display: { capable: true, ready: true, reason: '' },
  capture_status: { virtual_display_configured: false, displays: [], error: null },
  audio_enabled: false,
};

test('a settings fix saves exactly its keys on the click and says so', async () => {
  const [check] = problems(metadata);
  assert.equal(check.fix.label, 'Use a virtual display');
  const { sent, restore } = serve([
    ['/api/config', { status: true, restart_required: true, warning: null }],
    ['/api/metadata', { ...metadata, capture_status: { ...metadata.capture_status, virtual_display_configured: true } }],
  ]);
  let after = 'not called';
  try {
    await applyFix(check, (power) => (after = power));
  } finally {
    restore();
  }
  assert.deepEqual(sent[0], ['PATCH', '/api/config', { virtual_display_mode: 'per_client' }]);
  assert.equal(session.metadata.capture_status.virtual_display_configured, true);
  assert.equal(after, null);
  assert.equal(fixes.running, '');
  assert.equal(toasts.at(-1).message, 'Streams use a virtual display.');
});

test('the wake fix applies the listed changes, waits for the host’s result and hands back the new status', async () => {
  const status = (running, last, state = 'needs_setup') => ({
    status: true,
    wake: { primary: 'aa', targets: [{ mac: 'aa', kind: 'ethernet', connected: true, route: true, broadcasts: [], state, checks: [] }],
      system: { sleep: 's3', modern_standby: false, hibernate: false, fast_startup: 'off', shutdown_wake: 'unknown' } },
    idle_sleep: { state: 'off', minutes: 0, reason: null, remaining_seconds: null },
    last_sleep: null,
    fix: { running, changes: running || state === 'ready' ? [] : ['Realtek: allow this device to wake the computer'], skipped: [], error: null,
      fast_startup_off_offered: true, last },
  });
  const [check] = problems({ ...metadata, capture_status: { ...metadata.capture_status, displays: [{ name: 'D1' }] } }, status(false, { at: 5, errors: [] }));
  assert.equal(check.id, 'wake');
  const { sent, restore } = serve([
    ['/api/power/wake/fix', { status: true, changes: ['Realtek: allow this device to wake the computer'], skipped: [] }, 202],
    ['/api/power', status(false, { at: 9, errors: [] }, 'ready')],
  ]);
  let after = null;
  try {
    await applyFix(check, (power) => (after = power));
  } finally {
    restore();
  }
  // Fast Startup stays a choice of its own, in Wake settings.
  assert.deepEqual(sent[0], ['POST', '/api/power/wake/fix', { changes: ['Realtek: allow this device to wake the computer'], fast_startup_off: false }]);
  assert.equal(after.wake.targets[0].state, 'ready');
  assert.equal(toasts.at(-1).message, 'Devices can wake this PC now.');
  // Nothing runs a fix but a click.
  for (const name of ['pages/Overview.svelte', 'components/overview/HostReadiness.svelte']) {
    for (const call of source(name).matchAll(/applyFix\(/g)) {
      assert.match(source(name).slice(call.index - 40, call.index), /onclick=\{\(\) => (heroProblem && )?$/, name);
    }
  }
});

test('a console that loses the host shows Cannot connect over the page and comes back by itself', async () => {
  const { heartbeat, unreachable } = await server.ssrLoadModule('/src/lib/session.svelte.ts');
  const { ApiError, TimeoutError } = await server.ssrLoadModule('/src/lib/api.ts');
  // No answer at all is the host gone; a refusal is an answer.
  assert.equal(unreachable(new TypeError('Failed to fetch')), true);
  assert.equal(unreachable(new TimeoutError(20_000)), true);
  assert.equal(unreachable(new ApiError(500, 'boom')), false);
  assert.equal(unreachable(new ApiError(401, 'signed out')), false);
  const before = { state: session.state, metadata: session.metadata, pending: session.pending, live: session.live };
  session.state = 'signed-in';
  const realFetch = globalThis.fetch;
  globalThis.fetch = async () => { throw new TypeError('Failed to fetch'); };
  try {
    // The pairing poll and the sign-in check both go unanswered: the page stays, under the notice.
    await heartbeat();
    assert.equal(session.state, 'signed-in');
    assert.equal(session.reconnecting, true);
    assert.equal(session.error, 'Failed to fetch');
    await heartbeat();
    assert.equal(session.reconnecting, true);
  } finally {
    globalThis.fetch = realFetch;
  }
  // The host is back: the next poll finds the sign-in and the notice goes.
  const { sent, restore } = serve([
    ['/api/auth/status', { credentials_configured: true, authenticated: true }],
    ['/api/metadata', metadata],
    ['/api/clients/pending', { status: true, requests: [] }],
    ['/api/session/status', { activeSessions: 1, appRunning: true, appName: 'Portal 2' }],
  ]);
  try {
    await heartbeat();
    for (let wait = 0; wait < 50 && (sent.length < 4 || !session.live); wait++) await new Promise((resolve) => setTimeout(resolve, 1));
  } finally {
    restore();
  }
  assert.equal(session.reconnecting, false);
  assert.equal(session.state, 'signed-in');
  assert.equal(session.error, '');
  // The command bar's live badge is asked for with the rest, so it is right as soon as the host is back.
  assert.deepEqual(sent.map(([, url]) => url), ['/api/auth/status', '/api/metadata', '/api/clients/pending', '/api/session/status']);
  assert.deepEqual(session.live, { streams: 1, app: 'Portal 2' });
  // The shell covers the page instead of replacing it, so unsaved edits survive the outage.
  const app = source('App.svelte');
  assert.match(app, /return poll\(heartbeat, 3000\);/);
  assert.match(app, /\{#if session\.reconnecting\}<div class="reconnecting">\{@render cannotConnect\(\)\}<\/div>\{\/if\}/);
  assert.match(app, /<div class="shell" inert=\{session\.reconnecting\}/);
  Object.assign(session, before);
});
