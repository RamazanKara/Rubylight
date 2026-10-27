import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

globalThis.location = new URL('http://localhost/');
globalThis.window = new EventTarget();
const server = await createTestServer();
const { phases, step } = await server.ssrLoadModule('/src/components/maintenance/child-session.ts');
const { render } = await server.ssrLoadModule('svelte/server');
const Panel = (await server.ssrLoadModule('/src/components/maintenance/ChildSessionPanel.svelte')).default;
const { settings } = await server.ssrLoadModule('/src/lib/schema/display.ts');

const status = (overrides) => ({
  mode: 'child_session',
  label: 'Background streaming (child session): Windows Pro or higher',
  reason: 'background_mode = child_session (Windows edition Professional)',
  edition: 'Professional',
  missing: [],
  missing_error: null,
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
  assert.equal(step(status({ mode: 'virtual_display' })), 'off');
  assert.equal(step(status({ mode: 'inside_child_session' })), 'inside');
  assert.equal(step(status({ missing: ['Remote Desktop is off'] })), 'setup');
  assert.equal(step(status({ missing: null, missing_error: 'reading the account' })), 'setup', 'unread settings never offer Start');
  assert.equal(step(status({})), 'ready');
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

test('child sessions stay off by default and their display settings show only when used', () => {
  const mode = settings.find((setting) => setting.key === 'background_mode');
  assert.equal(mode.default, 'virtual_display');
  assert.match(mode.label, /Experimental/);
  const resolution = settings.find((setting) => setting.key === 'child_session_resolution');
  assert.equal(resolution.visibleWhen({}), false);
  assert.equal(resolution.visibleWhen({ background_mode: 'auto' }), true);
  assert.equal(resolution.visibleWhen({ background_mode: 'child_session' }), true);
});
