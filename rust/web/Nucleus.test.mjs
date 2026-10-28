import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

globalThis.location = new URL('http://localhost/');
globalThis.window = new EventTarget();
const server = await createTestServer();
const { endText, installStates, kickable, phaseOf, playerText, polls } = await server.ssrLoadModule('/src/components/maintenance/coop.ts');
const { render } = await server.ssrLoadModule('svelte/server');
const Panel = (await server.ssrLoadModule('/src/components/maintenance/NucleusPanel.svelte')).default;
const { toDraft, changes } = await server.ssrLoadModule('/src/components/devices/device.ts');

const status = (overrides) => ({
  mode: { setting: '', mode: 'nucleus', label: 'Background streaming (Nucleus Co-op)', reason: 'Nucleus Co-op runs background streams', legacy: false },
  install: { state: 'installed', path: 'C:\\ProgramData\\Butterpollo\\nucleus\\2.4.2', version: '2.4.2', error: null },
  available: true,
  reason: null,
  sessions: [],
  ...overrides,
});
const session = (overrides) => ({
  id: 3,
  kind: 'coop',
  app_id: 1234567,
  title: 'Portal 2',
  game: 'guid',
  phase: 'running',
  end: null,
  players: [
    { slot: 0, device: 'uuid-fold', name: 'Fold7', away: false },
    { slot: 1, device: null, name: null, away: false },
  ],
  ...overrides,
});
const html = (value) => render(Panel, { props: { initial: value } }).body;

test('the panel is labelled Experimental and loads with a skeleton', () => {
  const out = render(Panel, { props: {} }).body;
  assert.match(out, /Nucleus Co-op \(Experimental\)/);
  assert.doesNotMatch(out, /untested|Expected/i);
});

test('the mode, its reason and the install state show', () => {
  const out = html(status({}));
  assert.match(out, /Background streaming \(Nucleus Co-op\)/);
  assert.match(out, /Nucleus Co-op runs background streams/);
  assert.match(out, /Installed/);
  assert.doesNotMatch(out, /child session|Background mode/i);
  assert.match(out, /2\.4\.2/);
  assert.match(out, /No session runs/);
});

test('an unavailable mode, unavailable sessions and a failed install say why, loudly', () => {
  const out = html(
    status({
      mode: {
        setting: 'rdp',
        mode: 'unavailable',
        label: 'Background streaming: not available',
        reason: 'background_mode = rdp is not a mode: Nucleus Co-op is the only background mode, so remove the line or set it to nucleus',
        legacy: false,
      },
      available: false,
      reason: 'the host does not run as the service',
      install: { state: 'failed', path: null, version: null, error: 'The download has another SHA-256.' },
    }),
  );
  assert.match(out, /Background streaming: not available/);
  assert.match(out, /background_mode = rdp is not a mode/);
  assert.match(out, /Nucleus Co-op sessions are unavailable/);
  assert.match(out, /the host does not run as the service/);
  assert.match(out, /Installing Nucleus Co-op failed/);
  assert.match(out, /another SHA-256/);
});

test('a missing install says the first launch downloads it, and the legacy setting is explained', () => {
  const out = html(status({ install: { state: 'missing', path: null, version: null, error: null }, mode: { ...status({}).mode, setting: 'child_session', legacy: true } }));
  assert.match(out, /Not installed yet/);
  assert.match(out, /first background or co-op launch downloads Nucleus Co-op/);
  assert.match(out, /from an earlier build and reads as Nucleus Co-op/);
});

test('every install state and phase is spelled out, with a failure in danger', () => {
  for (const state of ['installed', 'missing', 'configured', 'installing', 'failed']) assert.ok(installStates[state]?.label, state);
  assert.equal(installStates.failed.tone, 'danger');
  for (const phase of ['starting', 'running', 'ended']) assert.ok(phaseOf(phase).label, phase);
  assert.equal(phaseOf('paused').label, 'paused', 'a newer phase still shows');
});

test('a co-op session lists its players with Remove for seated ones and End session', () => {
  const out = html(status({ sessions: [session({ players: [{ slot: 0, device: 'a', name: 'Fold7', away: true }, { slot: 1, device: null, name: null, away: false }] })] }));
  assert.match(out, /Portal 2/);
  assert.match(out, />Co-op</);
  assert.match(out, /Player 1: Fold7/);
  assert.match(out, /Away, slot kept/);
  assert.match(out, /Player 2: Free slot/);
  assert.equal(out.match(/>Remove</g)?.length, 1, 'a free slot has nothing to remove');
  assert.match(out, /End session/);
});

test('a background stream has no Remove, and an ended session offers nothing but says why', () => {
  const background = html(status({ sessions: [session({ kind: 'background', players: [{ slot: 0, device: 'a', name: 'Retroid', away: false }] })] }));
  assert.match(background, /Background stream/);
  assert.doesNotMatch(background, />Remove</);
  assert.match(background, /End session/);
  const ended = html(status({ sessions: [session({ phase: 'ended', end: { reason: 'crashed', detail: "Player 2's instance of Portal 2 exited" } })] }));
  assert.match(ended, /A game instance or Nucleus Co-op exited: Player 2/);
  assert.doesNotMatch(ended, />Remove<|End session/);
});

test('end and player texts', () => {
  assert.equal(endText({ reason: 'empty', detail: null }), 'Every player left and none came back in time');
  assert.equal(endText({ reason: 'failed', detail: 'no window in 60 s' }), 'Nucleus Co-op did not start the game: no window in 60 s');
  assert.equal(endText({ reason: 'melted', detail: null }), 'Ended (melted)');
  assert.equal(playerText({ slot: 2, device: 'x', name: null, away: false }), 'Player 3: Unnamed device');
  assert.equal(kickable(session({ phase: 'ended' }), { slot: 0, device: 'a', name: 'A', away: false }), false);
  assert.equal(kickable(session({}), { slot: 1, device: null, name: null, away: false }), false);
  assert.equal(kickable(session({}), { slot: 0, device: 'a', name: 'A', away: false }), true);
});

test('the panel polls only while a session runs or Nucleus Co-op downloads', () => {
  assert.equal(polls(status({})), false);
  assert.equal(polls(status({ sessions: [session({})] })), true);
  assert.equal(polls(status({ sessions: [session({ phase: 'ended' })] })), false);
  assert.equal(polls(status({ install: { state: 'installing', path: null, version: null, error: null } })), true);
});

test('Join co-op is off unless granted, reads as the host does, and clears with null', () => {
  const client = (extra) => ({ uuid: 'u', name: 'Fold7', perm: 0, enabled: true, connected: false, ...extra });
  assert.equal(toDraft(client({})).coop_join, false);
  assert.equal(toDraft(client({ coop_join: true })).coop_join, true);
  assert.equal(toDraft(client({ coop_join: 'true' })).coop_join, true, 'the host takes "true" too');
  for (const value of [1, '1', 'yes', false, null]) assert.equal(toDraft(client({ coop_join: value })).coop_join, false, String(value));
  const off = toDraft(client({}));
  const on = { ...off, coop_join: true };
  assert.deepEqual(changes(off, on), { coop_join: true });
  assert.deepEqual(changes(on, off), { coop_join: null });
});
