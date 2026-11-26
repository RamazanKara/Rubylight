import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { after, before, test } from 'node:test';
import { createTestServer } from './test-server.mjs';

// Quick Suspend on Home: the banner for a paused game, Suspend game on the running-app card and
// the live stage, and the wording of the host's answers.
globalThis.location = new URL('http://localhost/');
globalThis.window = new EventTarget();
let server, render, view, Banner, SuspendButton, RunningApp;
before(async () => {
  server = await createTestServer();
  ({ render } = await server.ssrLoadModule('svelte/server'));
  view = await server.ssrLoadModule('/src/lib/quick-suspend.ts');
  ({ default: Banner } = await server.ssrLoadModule('/src/components/session/SuspendedGameBanner.svelte'));
  ({ default: SuspendButton } = await server.ssrLoadModule('/src/components/session/SuspendButton.svelte'));
  ({ default: RunningApp } = await server.ssrLoadModule('/src/components/overview/RunningApp.svelte'));
});
after(() => server?.close());

const source = name => readFileSync(new URL(`./src/${name}`, import.meta.url), 'utf8');
const quick = (changes = {}) => ({
  status: true, state: 'ready', reason: 'none', message: null, app_title: 'Hollow Knight', suspended_count: 0,
  suspended_since_unix_ms: null, suspended_by: null, ...changes,
});
const now = 1_763_772_200_000;
const suspended = quick({ state: 'suspended', suspended_count: 2, suspended_since_unix_ms: now - 25 * 60_000, suspended_by: 'Pixel 9' });
const running = { status: true, activeSessions: 0, appRunning: true, appName: 'Hollow Knight', paused: true, running: false, app: null };

test('Suspend game shows only when the host says the game can be paused now', () => {
  assert.equal(view.canSuspend(quick()), true);
  for (const state of ['unavailable', 'suspended', 'resuming', 'refused', 'unknown']) assert.equal(view.canSuspend(quick({ state })), false, state);
  assert.equal(view.canSuspend(null), false);
  assert.match(render(SuspendButton, { props: { quick: quick() } }).body, />Suspend game</);
  // Off (the default) or no game: no button at all, never a disabled one.
  assert.doesNotMatch(render(SuspendButton, { props: { quick: quick({ state: 'unavailable', reason: 'disabled_for_app' }) } }).body, /Suspend/);
  assert.doesNotMatch(render(SuspendButton, { props: { quick: null } }).body, /Suspend/);
});

test('the banner names the game, how long it has been paused and the device that paused it', () => {
  assert.equal(view.isSuspended(suspended), true);
  assert.equal(view.isSuspended(quick({ state: 'resuming' })), true);
  assert.equal(view.isSuspended(quick()), false);
  assert.equal(view.suspendedCopy(suspended, now),
    'Hollow Knight is paused on this PC for 25 min from Pixel 9. It stays paused until you resume it, here or from any device.');
  assert.match(view.suspendedCopy(quick({ state: 'suspended', app_title: null }), now), /^The game is paused on this PC\. /);
  const html = render(Banner, { props: { quick: suspended } }).body;
  assert.match(html, /Game suspended/);
  assert.match(html, />Resume game</);
  assert.match(html, /Hollow Knight is paused on this PC/);
});

test('a refusal is a toast with the host’s reason, a pause or resume names the game', () => {
  const refused = quick({ state: 'refused', reason: 'anti_cheat_detected', message: 'Easy Anti-Cheat runs with this game, so Rubylight will not pause it.' });
  assert.deepEqual(view.outcome(refused, 'suspend'), { text: refused.message, ok: false });
  assert.deepEqual(view.outcome(suspended, 'suspend'), { text: 'Paused Hollow Knight.', ok: true });
  assert.deepEqual(view.outcome(quick(), 'resume'), { text: 'Resumed Hollow Knight.', ok: true });
  // A suspend the host answered with another state is no success.
  assert.equal(view.outcome(quick({ state: 'unavailable', message: null }), 'suspend').ok, false);
});

test('the running-app card says Suspended and offers Suspend game beside Close app', () => {
  const ready = render(RunningApp, { props: { status: running, quick: quick(), error: null, onchange() {} } }).body;
  assert.match(ready, />Suspend game<[\s\S]*>Close app</);
  assert.match(ready, />Paused</);
  const paused = render(RunningApp, { props: { status: running, quick: suspended, error: null, onchange() {} } }).body;
  assert.match(paused, />Suspended</);
  assert.doesNotMatch(paused, />Suspend game</);
});

test('Home reads the state with its poll, and the stage offers the same button', () => {
  const overview = source('pages/Overview.svelte');
  assert.match(overview, /api\.sessions\.suspendStatus\(\)/);
  assert.match(overview, /\{#if quick && isSuspended\(quick\)\}<SuspendedGameBanner /);
  assert.match(source('components/overview/LiveCommandStage.svelte'), /<SuspendButton \{quick\} \{onchange\} \/><Button variant="danger" icon="power"/);
  const api = source('lib/api.ts');
  for (const call of ["get<QuickSuspendStatus>('/api/session/suspend')", "post<QuickSuspendStatus>('/api/session/suspend'", "post<QuickSuspendStatus>('/api/session/resume'"]) {
    assert.ok(api.includes(call), call);
  }
});
