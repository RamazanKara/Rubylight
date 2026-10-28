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
