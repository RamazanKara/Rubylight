import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

globalThis.location = new URL('http://localhost/');
globalThis.window = new EventTarget();
const server = await createTestServer();
const { installStates } = await server.ssrLoadModule('/src/components/maintenance/coop.ts');
const { render } = await server.ssrLoadModule('svelte/server');
const Panel = (await server.ssrLoadModule('/src/components/maintenance/CoopHelpPanel.svelte')).default;
const { toDraft, changes, savingEndsStream } = await server.ssrLoadModule('/src/components/devices/device.ts');
const { PERM } = await server.ssrLoadModule('/src/lib/api.ts');
const { ACCESS_CHOICES, ACCESS_PERM } = await server.ssrLoadModule('/src/components/devices/access.ts');

const SIZE = 97_396_862;
const status = (overrides) => ({
  mode: { setting: '', mode: 'nucleus', label: 'Background streaming (Nucleus Co-op)', reason: 'Nucleus Co-op runs background streams' },
  install: { state: 'installed', path: 'C:\\ProgramData\\Butterpollo\\nucleus\\2.4.2', version: '2.4.2', error: null, release: { version: '2.4.2', size: SIZE }, received: null },
  available: true,
  reason: null,
  games: [],
  catalog_error: null,
  sessions: [],
  ...overrides,
});
const html = (value) => render(Panel, { props: { initial: value } }).body;

test('Help keeps co-op troubleshooting: the download and its check, the service, handlers and the build', () => {
  const out = html(status({}));
  assert.match(out, /Nucleus Co-op/);
  assert.match(out, /Installed/);
  assert.match(out, /2\.4\.2/);
  assert.match(out, /nucleus\\2\.4\.2/);
  assert.match(out, /Downloads Nucleus Co-op 2\.4\.2 \(97\u00a0MB\) from its GitHub release\. Its size and SHA-256 are checked/);
  assert.match(out, /Windows service/);
  assert.match(out, /handler hub/);
  assert.match(out, /a background stream of one player works\s+without one/);
  assert.match(out, /build 20348/);
  // Live games are on Home now, and nothing here says Experimental or names child sessions.
  assert.doesNotMatch(out, /End session|>Remove<|No session runs/);
  assert.doesNotMatch(out, /Experimental|untested|Expected|child session/i);
});

test('the panel loads with a skeleton', () => {
  const out = render(Panel, { props: {} }).body;
  assert.match(out, /Nucleus Co-op/);
});

test('a PC that can’t run co-op, a failed setup and handlers that didn’t load say why, loudly', () => {
  const out = html(
    status({
      available: false,
      reason: 'the host does not run as the service',
      install: { state: 'failed', path: null, version: null, error: 'The download has another SHA-256.', release: { version: '2.4.2', size: SIZE }, received: null },
      catalog_error: 'finding the signed-in user’s AppData folder failed',
    }),
  );
  assert.match(out, /Co-op can’t run on this PC/);
  // The host's reasons are clauses: the panel ends them as sentences, once.
  assert.match(out, /the host does not run as the service\./);
  assert.match(out, /AppData folder failed\./);
  assert.doesNotMatch(out, /SHA-256\.\./);
  assert.match(out, /Setting up co-op failed/);
  assert.match(out, /another SHA-256/);
  assert.match(out, /Try again/);
  assert.match(out, /role="alert"/);
  assert.match(out, /handlers didn’t load/);
});

test('every install state is spelled out, with a failure in danger', () => {
  for (const state of ['installed', 'missing', 'configured', 'installing', 'failed']) assert.ok(installStates[state]?.label, state);
  assert.equal(installStates.missing.label, 'Not set up yet');
  assert.equal(installStates.failed.status, 'error');
  assert.equal(installStates.missing.status, 'idle');
});

test('Help shows the co-op panel and no longer the sessions panel', async () => {
  const help = await readFile(new URL('./src/pages/Maintenance.svelte', import.meta.url), 'utf8');
  assert.match(help, /<CoopHelpPanel \/>/);
  assert.doesNotMatch(help, /NucleusPanel/);
});

test('“Can join co-op games” follows Launch apps until set, reads as the host does, and clears with null', () => {
  const client = (perm, extra) => ({ uuid: 'u', name: 'Fold7', perm, enabled: true, connected: true, ...extra });
  const play = ACCESS_PERM.play;
  // Never set: null, so it follows Launch apps.
  assert.equal(toDraft(client(play, {})).coop_join, null);
  assert.equal(toDraft(client(play, { coop_join: null })).coop_join, null);
  assert.equal(toDraft(client(0, { coop_join: true })).coop_join, true);
  assert.equal(toDraft(client(0, { coop_join: 'true' })).coop_join, true, 'the host takes "true" too');
  for (const value of [1, '1', 'yes', false]) assert.equal(toDraft(client(play, { coop_join: value })).coop_join, false, String(value));
  // Turned off on a device that launches apps: an explicit false goes to the host, and the stream ends.
  const follows = toDraft(client(play, {}));
  const off = { ...follows, coop_join: false };
  assert.deepEqual(changes(follows, off), { coop_join: false });
  assert.equal(savingEndsStream(true, follows, off), true);
  // Back to following clears the key.
  assert.deepEqual(changes(off, follows), { coop_join: null });
  assert.equal(savingEndsStream(true, off, follows), false);
  // Watch only and never set: taking Launch apps away also ends co-op, which the stream warning says.
  const watch = { ...follows, perm: ACCESS_PERM.view };
  assert.equal(savingEndsStream(true, follows, watch), true);
  assert.equal((ACCESS_PERM.play & PERM.LAUNCH_APPS) !== 0, true);
});

test('Play games says it joins co-op games', () => {
  assert.match(ACCESS_CHOICES.find((choice) => choice.id === 'play').hint, /join co-op games/);
});
