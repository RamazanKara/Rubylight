import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

globalThis.location = new URL('http://localhost/');
globalThis.window = new EventTarget();
const server = await createTestServer();
const { playTogether, pollsView, installProgress, downloadText, megabytes, COOP_LIMITS, AWAY_FROM_PC } = await server.ssrLoadModule('/src/lib/coop-view.ts');
const { render } = await server.ssrLoadModule('svelte/server');
const PlayTogether = (await server.ssrLoadModule('/src/components/library/PlayTogether.svelte')).default;
const { AppDraft } = await server.ssrLoadModule('/src/components/library/draft.svelte.ts');
const home = await server.ssrLoadModule('/src/lib/coop-view.ts');
const CoopCard = (await server.ssrLoadModule('/src/components/overview/CoopCard.svelte')).default;
const DeviceEditor = (await server.ssrLoadModule('/src/components/devices/DeviceEditor.svelte')).default;
const { api, PERM } = await server.ssrLoadModule('/src/lib/api.ts');
const { ACCESS_PERM } = await server.ssrLoadModule('/src/components/devices/access.ts');

const SIZE = 97_396_862;
/** `GET /api/coop` as the host answers it: Nucleus Co-op available, set up, Portal 2 ready. */
const status = (overrides = {}, game = {}) => ({
  mode: { setting: '', mode: 'nucleus', label: 'Background streaming (Nucleus Co-op)', reason: 'Nucleus Co-op runs background streams' },
  install: {
    state: 'installed',
    path: 'C:\\ProgramData\\Butterpollo\\nucleus\\2.4.2',
    version: '2.4.2',
    error: null,
    release: { version: '2.4.2', size: SIZE },
    received: null,
  },
  available: true,
  reason: null,
  games: [
    { app: 'portal', name: 'Portal 2', status: 'ready', reason: null, handler: 'Portal 2', players: 2, entry: 'Portal 2 (Co-op)', ...game },
    { app: 'other', name: 'Other', status: 'no_handler', reason: 'No Nucleus Co-op handler for other.exe: add one in Nucleus Co-op\'s handler hub', handler: null, players: null, entry: 'Other (Co-op)' },
  ],
  catalog_error: null,
  sessions: [],
  ...overrides,
});
const install = (fields) => ({ ...status().install, path: null, version: null, ...fields });
const fixtures = {
  hidden: status({}, { app: 'elsewhere' }),
  unavailable: status({ available: false, reason: 'Nucleus Co-op sessions need the host to run as the Rubylight Windows service, which starts Nucleus Co-op as the signed-in user' }),
  setup: status({ install: install({ state: 'missing' }) }, { status: 'unknown', handler: null, players: null }),
  installing: status({ install: install({ state: 'installing', received: 41_000_000 }) }, { status: 'unknown' }),
  failed: status(
    { install: install({ state: 'failed', error: 'Nucleus Co-op is not ready: The Nucleus Co-op download\'s SHA-256 is 00ff, not version 2.4.2\'s ed91; it was deleted.' }) },
    { status: 'unknown' },
  ),
  broken: status({ catalog_error: 'finding the signed-in user\'s AppData folder failed' }, { status: 'unknown' }),
  checking: status({}, { status: 'unknown', handler: null, players: null }),
  handler: status({}, { status: 'no_handler', handler: null, players: null }),
  added: status({}, { status: 'not_added', handler: null, players: null }),
  ready: status(),
  launcher: status({ install: install({ state: 'missing' }) }, { name: 'Counter-Strike 2', status: 'launcher', handler: null, players: null }),
};
const expected = {
  hidden: 'hidden',
  unavailable: 'unavailable',
  setup: 'setup',
  installing: 'installing',
  failed: 'failed',
  broken: 'broken',
  checking: 'checking',
  handler: 'handler',
  added: 'handler',
  ready: 'ready',
  launcher: 'launcher',
};
const html = (value, app = 'portal') => render(PlayTogether, { props: { uuid: app, draft: new AppDraft({ name: 'Portal 2', uuid: app }), initial: value } }).body;

test('each /api/coop fixture is one state of the Play together panel', () => {
  for (const [name, fixture] of Object.entries(fixtures)) assert.equal(playTogether(fixture, 'portal').kind, expected[name], name);
});

test('not set up: the offer, and the pinned download named before it runs', () => {
  const view = playTogether(fixtures.setup, 'portal');
  assert.equal(view.text, 'Play this game together, or play it while someone uses this PC.');
  assert.equal(view.download, 'Downloads Nucleus Co-op 2.4.2 (97\u00a0MB) from its GitHub release.');
  const out = html(fixtures.setup);
  assert.match(out, /Play together/);
  assert.match(out, /Set up co-op/);
  assert.match(out, /97\u00a0MB/);
  // An older host without the release still names the source.
  assert.equal(downloadText({ install: { state: 'missing' } }), 'Downloads Nucleus Co-op from its GitHub release.');
});

test('installing shows the progress, then the check and unpack', () => {
  assert.deepEqual(installProgress(fixtures.installing), { label: 'Downloading Nucleus Co-op… 41\u00a0MB of 97\u00a0MB', percent: 42 });
  const done = status({ install: install({ state: 'installing', received: SIZE }) });
  assert.deepEqual(installProgress(done), { label: 'Checking the download and unpacking it…', percent: 100 });
  assert.equal(installProgress(status({ install: install({ state: 'installing', release: undefined }) })).percent, null);
  const out = html(fixtures.installing);
  assert.match(out, /role="progressbar"/);
  assert.match(out, /aria-valuenow="42"/);
  assert.equal(megabytes(SIZE), '97\u00a0MB');
});

test('a refused download is loud, gives the host’s reason and can be tried again', () => {
  const out = html(fixtures.failed);
  assert.match(out, /Setting up co-op failed/);
  assert.match(out, /SHA-256 is 00ff/);
  assert.match(out, /it was deleted/);
  assert.match(out, /Try again/);
  assert.match(out, /role="alert"/);
});

test('no handler: what is missing, the button that opens Nucleus Co-op on the PC, and the steps from a phone', () => {
  const view = playTogether(fixtures.handler, 'portal');
  assert.equal(view.text, 'Nucleus Co-op needs a handler for Portal 2.');
  assert.match(view.steps, /search its handler hub for Portal 2/);
  const out = html(fixtures.handler);
  assert.match(out, /Open Nucleus Co-op on this PC/);
  assert.match(out, /Using this console away from the PC\?/);
  assert.ok(out.includes(AWAY_FROM_PC));
  const added = playTogether(fixtures.added, 'portal');
  assert.equal(added.text, 'Nucleus Co-op has a handler for Portal 2, but the game isn’t added yet.');
  assert.match(added.steps, /add Portal 2 once/);
});

test('ready: the players, how to start it, the library switch on by default, and the limits', () => {
  const view = playTogether(fixtures.ready, 'portal');
  assert.equal(view.players, 'Up to 2 players.');
  assert.equal(view.entry, 'Portal 2 (Co-op)');
  assert.match(view.start, /Start “Portal 2 \(Co-op\)” from Nightjar or Moonlight/);
  assert.equal(playTogether(status({}, { players: 1 }), 'portal').players, 'Up to 1 player.');
  const out = html(fixtures.ready);
  assert.match(out, /Show “Portal 2 \(Co-op\)” in the library/);
  assert.match(out, /role="switch"[^>]*checked/);
  assert.ok(out.includes(COOP_LIMITS));
  // Turned off, the app saves coop-entry false; back on, the key goes again.
  const draft = new AppDraft({ name: 'Portal 2', uuid: 'portal' });
  assert.equal(draft.flag('coop-entry'), true);
  draft.setFlag('coop-entry', false);
  assert.equal(draft.record()['coop-entry'], false);
  draft.setFlag('coop-entry', true);
  assert.equal('coop-entry' in draft.record(), false);
});

test('a game a launcher starts says how to make co-op possible, with nothing to download or open', () => {
  const view = playTogether(fixtures.launcher, 'portal');
  assert.equal(view.text, 'Co-op finds a game by its .exe, and Counter-Strike 2 starts through a launcher. To play it together, set the command above to the game’s .exe.');
  const out = html(fixtures.launcher);
  assert.doesNotMatch(out, /Set up co-op|Open Nucleus Co-op/);
});

test('a PC that can’t run co-op says why and offers no download', () => {
  const out = html(fixtures.unavailable);
  assert.match(out, /Co-op can’t run on this PC/);
  assert.match(out, /Rubylight Windows service/);
  assert.doesNotMatch(out, /Set up co-op/);
  // The host's reason is a clause; the panel ends it as a sentence, once.
  assert.match(playTogether(fixtures.unavailable, 'portal').reason, /signed-in user\.$/);
  assert.equal(playTogether(status({ available: false, reason: 'No service.' }), 'portal').reason, 'No service.');
});

test('an app without an .exe, or an older host, shows no panel', () => {
  assert.equal(html(fixtures.hidden).trim().replace(/<!--[^>]*-->/g, ''), '');
  assert.equal(playTogether(status({ games: undefined }), 'portal').kind, 'hidden');
  assert.equal(playTogether(status({}, { status: 'no_executable' }), 'portal').kind, 'hidden');
});

test('the panel polls only while something changes on its own', () => {
  const polling = Object.entries(fixtures).filter(([, fixture]) => pollsView(playTogether(fixture, 'portal'))).map(([name]) => name);
  assert.deepEqual(polling, ['installing', 'checking', 'handler', 'added']);
});

test('no panel text says Experimental or names a child session', () => {
  for (const fixture of Object.values(fixtures)) {
    const out = html(fixture);
    assert.doesNotMatch(out, /Experimental|untested|child session/i);
  }
});

// ---------- Home: the live co-op card ----------


const PLAY = ACCESS_PERM.play;
const coopSession = (players, overrides = {}) => ({
  id: 7,
  kind: 'coop',
  app_id: 242170942,
  title: 'Portal 2',
  game: 'guid',
  phase: 'running',
  end: null,
  players,
  ...overrides,
});
const seat = (slot, device, name, away = false, rejoin_in = null) => ({ slot, device, name, away, rejoin_in });
const device = (uuid, name, perm = PLAY, extra = {}) => ({ uuid, name, perm, enabled: true, connected: false, ...extra });
const running = coopSession([seat(0, 'fold', 'Fold7'), seat(1, 'tv', 'Living room TV', true, 105), seat(2, null, null), seat(3, null, null)]);
const paired = [
  device('fold', 'Fold7'),
  device('tv', 'Living room TV'),
  device('retroid', 'Retroid'),
  device('laptop', 'Laptop', ACCESS_PERM.view),
  device('deck', 'Steam Deck', PLAY, { coop_join: false }),
  device('old', 'Old phone', PLAY, { enabled: false }),
];
const card = (session = running, clients = paired, extra = {}) =>
  render(CoopCard, { props: { session, clients, at: Date.now(), ...extra } }).body;

test('Home shows one card per running co-op game, never for a background stream or an ended game', () => {
  const live = home.liveCoop(status({ sessions: [running, coopSession([], { id: 8, kind: 'background' }), coopSession([], { id: 9, phase: 'ended' })] }));
  assert.deepEqual(live.map((session) => session.id), [7]);
  assert.deepEqual(home.liveCoop(null), []);
});

test('the card shows the game, each player with their slot, who is away and the free slots', () => {
  assert.equal(home.headcount(running), '2 of 4 players · 2 free slots');
  assert.equal(home.headcount(coopSession([seat(0, 'a', 'A')])), '1 of 1 player · no free slot');
  const out = card();
  assert.match(out, /Portal 2/);
  assert.match(out, /Co-op · 2 of 4 players · 2 free slots/);
  assert.match(out, /Player 1/);
  assert.match(out, /Fold7/);
  assert.match(out, /Playing/);
  // The badge says Away; the countdown sits beside it as text, so a phone wraps it instead of the pill.
  assert.match(out, /badge warn[^>]*>(<!--[^>]*-->|\s)*Away(<!--[^>]*-->)*<\/span>(<!--[^>]*-->)*<span class="keeps[^"]*">keeps the slot for 1:45<\/span>/);
  assert.equal(out.match(/Free slot/g)?.length, 2);
  assert.equal(out.match(/>Remove</g)?.length, 2, 'only seated players, away ones too');
  assert.match(out, /End co-op/);
});

test('the away time counts down between polls and never goes below zero', () => {
  assert.equal(home.seats(running, 10)[1].away, 'Away, keeps the slot for 1:35');
  assert.equal(home.seats(running, 500)[1].away, 'Away, keeps the slot for 0:00');
  assert.equal(home.seats(coopSession([seat(0, 'a', 'A', true)]))[0].away, 'Away, keeps the slot for 2 minutes', 'an older host');
  assert.equal(home.seats(running)[0].away, null);
  assert.equal(home.seats(running)[0].keeps, null);
  assert.equal(home.seats(running, 10)[1].keeps, 'keeps the slot for 1:35');
  assert.equal(home.clock(65), '1:05');
  assert.equal(home.clock(9.2), '0:10');
});

test('the invite list leaves out seated, turned-off and watch-only devices, and says who can already join', () => {
  const list = home.invitees(running, paired);
  assert.deepEqual(
    list.map((d) => [d.name, d.canJoin]),
    [
      ['Retroid', true],
      ['Steam Deck', false],
    ],
  );
  assert.deepEqual(home.invitees(coopSession([seat(0, 'fold', 'Fold7')]), paired), [], 'a full game invites no one');
  const out = card();
  assert.match(out, /Invite a player/);
  assert.match(out, /Can join/);
  assert.match(out, /Let Steam Deck join/);
  // Watch only can't launch the co-op entry, so letting it join would change nothing.
  assert.doesNotMatch(out, /Laptop/);
  assert.doesNotMatch(out, /Let Retroid join|Let Fold7 join|Old phone/);
  assert.match(out, /Open Nightjar or Moonlight on it and pick “Portal 2 \(Co-op\)”\./);
  assert.match(out, /href="\/devices"[^>]*>.*Pair a new device/s);
  // While the devices load, or fail to, the card says so.
  assert.match(card(running, null), /Loading paired devices/);
  assert.match(card(running, null, { clientsError: 'timed out' }), /Can’t list the paired devices: timed out/);
});

test('Remove and End co-op confirm first, with the brief’s words', () => {
  const tv = home.seats(running)[1];
  assert.deepEqual(home.removeConfirmation(tv), {
    title: 'Remove Living room TV?',
    message: 'Living room TV leaves the game and can’t rejoin this session.',
    confirm: 'Remove',
    danger: true,
  });
  assert.deepEqual(home.endConfirmation(running), {
    title: 'End co-op in Portal 2?',
    message: 'Ends every player’s stream and closes Portal 2 on this PC.',
    confirm: 'End co-op',
    danger: true,
  });
});

test('Remove, End co-op and Let join post the right bodies', async () => {
  const seen = [];
  globalThis.fetch = async (path, init) => {
    // The CSRF token comes first, once.
    if (path === '/api/csrf-token') return new Response('{"csrf_token":"t"}', { status: 200 });
    seen.push({ path, method: init.method, body: JSON.parse(init.body) });
    return new Response('{}', { status: 200, headers: { 'content-type': 'application/json' } });
  };
  try {
    await api.coop.kick(7, 1);
    await api.coop.end(7);
    await api.clients.update({ uuid: 'laptop', coop_join: true });
  } finally {
    delete globalThis.fetch;
  }
  assert.deepEqual(seen, [
    { path: '/api/coop/kick', method: 'POST', body: { session: 7, slot: 1 } },
    { path: '/api/coop/end', method: 'POST', body: { session: 7 } },
    { path: '/api/clients/update', method: 'POST', body: { uuid: 'laptop', coop_join: true } },
  ]);
});

test('“Can join co-op games” follows Launch apps until set, as the host decides', () => {
  assert.equal(home.joinsCoop(PLAY, undefined), true);
  assert.equal(home.joinsCoop(PLAY, null), true);
  assert.equal(home.joinsCoop(PLAY, true), true);
  assert.equal(home.joinsCoop(PLAY, 'true'), true);
  assert.equal(home.joinsCoop(ACCESS_PERM.view, undefined), false);
  assert.equal(home.joinsCoop(PLAY, false), false, 'an explicit false refuses');
  // Joining launches the co-op entry: without Launch apps a stored true can't let it in.
  assert.equal(home.joinsCoop(ACCESS_PERM.view, true), false);
  assert.equal(home.joinsCoop(0, 'true'), false);
  assert.equal(home.joinsCoop(PLAY, 1), false);
  assert.equal((PLAY & PERM.LAUNCH_APPS) !== 0, true);
});

test('a device’s page shows “Can join co-op games”, on for Play games and off when turned off', () => {
  const page = (client) => render(DeviceEditor, { props: { client } }).body;
  const on = page(device('fold', 'Fold7'));
  assert.match(on, /Can join co-op games/);
  assert.match(on, /role="switch"[^>]*checked|checked[^>]*role="switch"/);
  const off = page(device('deck', 'Steam Deck', PLAY, { coop_join: false }));
  assert.match(off, /Can join co-op games/);
  assert.doesNotMatch(off.slice(off.indexOf('Can join co-op games') - 400, off.indexOf('Can join co-op games')), /checked/);
  assert.doesNotMatch(on, /Join co-op:/, 'one control, not a second checkbox');
  // Watch only can't launch the co-op entry: the switch is off and says what it needs, even if stored true.
  const watching = page(device('laptop', 'Laptop', ACCESS_PERM.view, { coop_join: true }));
  const at = watching.indexOf('Can join co-op games');
  const toggle = watching.slice(watching.lastIndexOf('<label', at), at);
  assert.match(toggle, /disabled/);
  assert.doesNotMatch(toggle, /checked/);
  assert.match(watching, /Needs Launch apps, which Play games and Everything include\./);
});

test('every co-op text names the permission the way the device page does', () => {
  assert.match(home.COOP_LIMITS, /“Can join co-op games”/);
  assert.doesNotMatch(home.COOP_LIMITS, /Join co-op is/);
  assert.doesNotMatch(card(), /Experimental|untested|child session/i);
});
