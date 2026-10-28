import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

globalThis.location = new URL('http://localhost/');
globalThis.window = new EventTarget();
const server = await createTestServer();
const { phases, statusLoader, step } = await server.ssrLoadModule('/src/components/maintenance/child-session.ts');
const { render } = await server.ssrLoadModule('svelte/server');
const Panel = (await server.ssrLoadModule('/src/components/maintenance/ChildSessionPanel.svelte')).default;
const { settings } = await server.ssrLoadModule('/src/lib/schema/display.ts');

const status = (overrides) => ({
  mode: 'child_session',
  label: 'Background streaming (child session)',
  reason: 'chosen in the settings (Windows edition Professional)',
  edition: 'Professional',
  missing: [],
  missing_error: null,
  unavailable: null,
  set_up: true,
  display: { width: 1920, height: 1080, scale: 100 },
  display_error: null,
  viewer: null,
  host_port: 48989,
  problem: null,
  limits: [],
  ...overrides,
});

test('the panel offers setup, then start, and only when Background mode asks for a child session', () => {
  assert.equal(step(status({ mode: 'nucleus' })), 'off');
  assert.equal(step(status({ mode: 'unavailable' })), 'refused', 'a chosen child session this PC lacks says why');
  assert.equal(step(status({ mode: 'inside_child_session' })), 'inside');
  assert.equal(step(status({ missing: ['Remote Desktop is off'] })), 'setup');
  assert.equal(step(status({ missing: null, missing_error: 'reading the account' })), 'setup', 'unread settings never offer Start');
  assert.equal(step(status({})), 'ready');
  const unavailable = 'Child sessions are set up, but this host runs without administrator rights';
  assert.equal(step(status({ unavailable })), 'unavailable', 'a host without administrator rights never offers Start');
  assert.equal(step(status({ unavailable, missing: ['Remote Desktop is off'] })), 'unavailable', 'nor Setup');
  assert.equal(step(status({ unavailable, mode: 'nucleus' })), 'off');
});

test('every viewer phase is spelled out, with a failure in danger', () => {
  for (const phase of ['idle', 'needs_password', 'connecting', 'connected', 'reconnecting', 'signing_out', 'sign_in_failed', 'signed_out']) {
    assert.ok(phases[phase]?.label, phase);
  }
  assert.equal(phases.sign_in_failed.tone, 'danger');
  assert.equal(phases.connected.tone, 'ok');
});

test('the panel is labelled Experimental and loads with a skeleton', () => {
  const out = render(Panel, { props: {} }).body;
  assert.match(out, /Background child session \(Experimental\)/);
  assert.doesNotMatch(out, /untested|Expected/i);
});

test('no setting chooses a background mode: Nucleus Co-op is the only one', () => {
  for (const key of ['background_mode', 'child_session_resolution', 'child_session_scale']) {
    assert.equal(settings.find((setting) => setting.key === key), undefined, key);
  }
});

test('a poll tick waits for the status load in flight, and a slow load never overwrites a newer one', async () => {
  const pending = [];
  const applied = [];
  const failed = [];
  const loader = statusLoader(
    () => new Promise((resolve, reject) => pending.push({ resolve, reject })),
    (value) => applied.push(value),
    (error) => failed.push(error),
  );
  const slow = loader.load();
  await loader.tick();
  assert.equal(pending.length, 1, 'no second status request while one is in flight');
  // An action's own load still goes out, and its answer wins over the slower one.
  const asked = loader.load();
  assert.equal(pending.length, 2);
  pending[1].resolve('newer');
  await asked;
  pending[0].resolve('older');
  await slow;
  assert.deepEqual(applied, ['newer']);
  // Nothing in flight: the next tick loads.
  const next = loader.tick();
  assert.equal(pending.length, 3);
  pending[2].reject(new Error('host gone'));
  await next;
  assert.equal(failed.length, 1);
  assert.deepEqual(applied, ['newer']);
});
