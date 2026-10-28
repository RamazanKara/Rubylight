import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

const server = await createTestServer({ configFile: false });
const power = await server.ssrLoadModule('/src/lib/power.ts');
const { checkName, checkPowerStatus, checkState, idleSleep, readiness, systemNotes } = power;

const wired = {
  mac: 'aa:bb:cc:dd:ee:01', kind: 'ethernet', connected: true, route: true, broadcasts: ['192.168.4.255'], state: 'ready',
  checks: [{ id: 'wake_enabled', state: 'ok' }, { id: 'magic_packet', state: 'ok' }, { id: 'magic_packet_only', state: 'warning' }],
};
const wifi = { mac: 'aa:bb:cc:dd:ee:02', kind: 'wifi', connected: false, route: false, broadcasts: [], state: 'experimental', checks: [] };
// As `GET /api/power` sends it (host/src/wake.rs console_status).
const status = (changes = {}) => ({
  status: true,
  wake: {
    revision: 1, primary: wired.mac, targets: [wired, wifi],
    system: { sleep: 's3', modern_standby: false, hibernate: false, fast_startup: 'inactive', shutdown_wake: 'unknown' },
    ...changes.wake,
  },
  idle_sleep: { state: 'off', minutes: 0, reason: null, remaining_seconds: null, ...changes.idle_sleep },
  last_sleep: null,
  fix: { running: false, changes: [], skipped: [], error: null, fast_startup_off_offered: false, last: null, ...changes.fix },
});

test('readiness follows the adapter devices are sent, and says what to do', () => {
  assert.equal(readiness(status()).status, 'ready');
  assert.match(readiness(status()).text, /through the wired adapter aa:bb:cc:dd:ee:01/);
  const needs = readiness(status({ wake: { targets: [{ ...wired, state: 'needs_setup' }] } }));
  assert.deepEqual([needs.status, needs.tone], ['warning', 'warn']);
  assert.match(needs.text, /Apply the changes below/);
  const onlyWifi = readiness(status({ wake: { primary: wifi.mac, targets: [{ ...wifi, connected: true }] } }));
  assert.equal(onlyWifi.label, 'Depends on Wi-Fi');
  assert.doesNotMatch(onlyWifi.text, /Experimental/);
  assert.equal(readiness(status({ wake: { primary: null } })).label, 'Not connected');
  assert.equal(readiness(status({ wake: { targets: [], primary: null } })).tone, 'danger');
  // A state from a newer host is not shown as ready.
  assert.equal(readiness(status({ wake: { targets: [{ ...wired, state: 'sleepy' }] } })).label, 'Unknown');
});

test('checks use the names Device Manager shows, and unknown ones stay visible', () => {
  assert.equal(checkName('magic_packet_only'), 'Only allow a magic packet to wake the computer');
  assert.equal(checkName('future_check'), 'future_check');
  assert.deepEqual(checkState({ id: 'wake_enabled', state: 'error' }), { word: 'Needs fixing', tone: 'danger' });
  assert.equal(checkState({ id: 'wake_enabled', state: 'later' }).word, 'Unreadable');
});

test('sleep after idle shows its countdown, and why it waits or cannot run', () => {
  assert.deepEqual(idleSleep(status()), { title: 'Off', tone: 'neutral' });
  const armed = idleSleep(status({ idle_sleep: { state: 'armed', minutes: 30, remaining_seconds: 720, reason: 'waiting for console activity' } }));
  assert.equal(armed.title, 'After 30 min idle');
  assert.equal(armed.text, 'Sleeps in 12 min if nothing changes (counted from the last use of this console).');
  assert.equal(idleSleep(status({ idle_sleep: { state: 'armed', minutes: 30, remaining_seconds: 0 } })).text, 'The PC goes to sleep now.');
  const blocked = idleSleep(status({ idle_sleep: { state: 'blocked', minutes: 30, reason: 'a stream is active or starting' } }));
  assert.equal(blocked.text, 'A stream is active or starting.');
  // A hand-edited value the host refuses is an error, with the host's words.
  const invalid = idleSleep(status({ idle_sleep: { state: 'blocked', minutes: 0, reason: 'sleep_after_idle_minutes = 3 is invalid' } }));
  assert.deepEqual([invalid.title, invalid.tone], ['Not working', 'danger']);
  assert.match(invalid.text, /sleep_after_idle_minutes = 3 is invalid/);
  const modern = idleSleep(status({ idle_sleep: { state: 'unsupported', minutes: 30, reason: 'this PC uses Modern Standby, which Windows lets no service enter' } }));
  assert.equal(modern.title, 'Unavailable on this PC');
  assert.equal(idleSleep(status({ idle_sleep: { state: 'dozing', minutes: 30 } })).tone, 'warn');
});

test('system notes explain Fast Startup, Modern Standby and firmware wake', () => {
  assert.deepEqual(systemNotes(status()), []);
  const notes = systemNotes(status({ wake: { system: { sleep: 'none', modern_standby: true, hibernate: true, fast_startup: 'active', shutdown_wake: 'firmware_unknown' } } }));
  assert.equal(notes.length, 3);
  assert.match(notes[0], /Modern Standby/);
  assert.match(notes[1], /Fast Startup is on/);
  assert.match(notes[2], /firmware/);
});

test('an incomplete power status is an error, not an empty page', () => {
  assert.equal(checkPowerStatus(status()).wake.targets.length, 2);
  // What an older host's fallback, or another endpoint, would answer.
  for (const reply of [{ status: true, available: false, enabled: false }, null, [], { ...status(), fix: null }, { ...status(), idle_sleep: {} }]) {
    assert.throws(() => checkPowerStatus(reply), /incomplete power status/);
  }
});

test("an adapter check's badge keeps its width, so it stays inside the card on a phone", () => {
  const panel = readFileSync(new URL('./src/components/maintenance/PowerPanel.svelte', import.meta.url), 'utf8');
  const rule = panel.match(/\.checks \{([^}]*)\}/);
  assert.ok(rule);
  assert.match(rule[1], /grid-template-columns: fit-content\(16em\) minmax\(max-content, 1fr\)/);
});
