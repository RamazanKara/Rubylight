import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

globalThis.location = new URL('http://localhost/');
globalThis.window = new EventTarget();
const server = await createTestServer();
const { installStates, installPill } = await server.ssrLoadModule('/src/components/maintenance/coop.ts');
const { render } = await server.ssrLoadModule('svelte/server');
const Panel = (await server.ssrLoadModule('/src/components/maintenance/CoopHelpPanel.svelte')).default;
const { toDraft, changes, savingEndsStream } = await server.ssrLoadModule('/src/components/devices/device.ts');
const { PERM } = await server.ssrLoadModule('/src/lib/api.ts');
const { ACCESS_CHOICES, ACCESS_PERM } = await server.ssrLoadModule('/src/components/devices/access.ts');

const PAGE = 'https://github.com/SplitScreen-Me/splitscreenme-nucleus/releases/tag/v2.4.2';
const release = { version: '2.4.2', page: PAGE };
const status = (overrides) => ({
  mode: { setting: '', mode: 'nucleus', label: 'Background streaming (Nucleus Co-op)', reason: 'Nucleus Co-op runs background streams' },
  install: { state: 'installed', path: 'C:\\ProgramData\\Butterpollo\\nucleus\\2.4.2', chosen: '', version: '2.4.2', problem: null, error: null, release },
  available: true,
  reason: null,
  games: [],
  catalog_error: null,
  sessions: [],
  ...overrides,
});
const html = (value) => render(Panel, { props: { initial: value } }).body;

test('Help keeps co-op troubleshooting: the folder and its check, the service, handlers and the build', () => {
  const out = html(status({}));
  assert.match(out, /Nucleus Co-op/);
  // An earlier Rubylight's install keeps working.
  assert.match(out, /Earlier install/);
  assert.match(out, /2\.4\.2/);
  assert.match(out, /nucleus\\2\.4\.2/);
  // The release's page and the folder to choose; Rubylight downloads nothing.
  assert.ok(out.includes(`href="${PAGE}" target="_blank" rel="noreferrer">Download Nucleus Co-op 2.4.2`), out);
  assert.match(out, /Use this folder/);
  assert.match(out, /Rubylight downloads nothing: it runs only Nucleus Co-op 2\.4\.2, checks its NucleusCoop\.exe/);
  assert.doesNotMatch(out, /SHA-256|Try again|Downloading/);
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

test('a PC that can’t run co-op, a folder it can’t run from and handlers that didn’t load say why, loudly', () => {
  const why = 'Rubylight doesn\'t run Nucleus Co-op from D:\\Nucleus: it runs elevated, so only administrators may change its folder';
  const out = html(
    status({
      available: false,
      reason: 'the host does not run as the service',
      install: { state: 'failed', path: null, chosen: 'D:\\Nucleus', version: null, problem: 'untrusted', error: `${why}.`, release },
      catalog_error: 'finding the signed-in user’s AppData folder failed',
    }),
  );
  assert.match(out, /Co-op can’t run on this PC/);
  // The host's reasons are clauses: the panel ends them as sentences, once.
  assert.match(out, /the host does not run as the service\./);
  assert.match(out, /AppData folder failed\./);
  assert.doesNotMatch(out, /its folder\.\./);
  assert.match(out, /Rubylight won’t run Nucleus Co-op from the folder chosen/);
  assert.match(out, /Folder refused/);
  assert.match(out, /only administrators may change its folder/);
  assert.match(out, /value="D:\\Nucleus"/);
  assert.match(out, /role="alert"/);
  assert.match(out, /handlers didn’t load/);
});

test('every install state is spelled out: found, wrong version and missing apart, a failure in danger', () => {
  for (const state of ['installed', 'missing', 'configured', 'failed']) assert.ok(installStates[state]?.label, state);
  assert.equal(installStates.installing, undefined);
  assert.equal(installStates.missing.label, 'Not set up yet');
  assert.equal(installStates.failed.status, 'error');
  assert.equal(installStates.missing.status, 'idle');
  const pill = (fields) => installPill({ state: 'failed', path: null, version: null, error: 'why', ...fields });
  assert.deepEqual(installPill({ state: 'configured', path: 'C:\\Program Files\\Nucleus', version: '2.4.2', error: null }), { label: 'Found', status: 'ready' });
  assert.deepEqual(pill({ problem: 'version' }), { label: 'Wrong version', status: 'error' });
  assert.deepEqual(pill({ problem: 'missing' }), { label: 'Not found', status: 'error' });
  assert.deepEqual(pill({ problem: 'untrusted' }), { label: 'Folder refused', status: 'error' });
  assert.deepEqual(pill({ problem: null }), { label: 'Can’t run', status: 'error' });
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
