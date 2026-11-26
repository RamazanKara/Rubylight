import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { after, before, test } from 'node:test';
import { createTestServer } from './test-server.mjs';

// Rubylight 7.0's pairing cinema: the guided frame around the pairing panel.
globalThis.location = new URL('http://localhost/');
globalThis.window = new EventTarget();
let server, render, steps, Cinema, AccessChoice;
before(async () => {
  server = await createTestServer();
  ({ render } = await server.ssrLoadModule('svelte/server'));
  steps = await server.ssrLoadModule('/src/components/devices/pairing-steps.ts');
  ({ default: Cinema } = await server.ssrLoadModule('/src/components/devices/PairingCinema.svelte'));
  ({ default: AccessChoice } = await server.ssrLoadModule('/src/components/devices/AccessChoice.svelte'));
});
after(() => server?.close());

const source = name => readFileSync(new URL(`./src/components/devices/${name}`, import.meta.url), 'utf8');

test('the step follows the host: a code, a PIN request that waits, a device that paired', () => {
  assert.equal(steps.pairingStage({}), 'code');
  assert.equal(steps.pairingStage({ pending: ['Steam Deck'] }), 'confirm');
  assert.equal(steps.pairingStage({ pending: ['Steam Deck'], off: true }), 'code', 'no PIN is taken while pairing is off');
  assert.equal(steps.pairingStage({ pending: ['Steam Deck'], welcome: 'Pixel 9' }), 'ready');
  assert.deepEqual(steps.pairingSteps('code').map(step => step.state), ['now', 'next', 'next']);
  assert.deepEqual(steps.pairingSteps('confirm').map(step => step.state), ['done', 'now', 'next']);
  assert.deepEqual(steps.pairingSteps('ready').map(step => step.state), ['done', 'done', 'done']);
  assert.deepEqual(steps.pairingSteps('code').map(step => step.title), ['Show the code', 'Confirm the device', 'Ready to play']);
  // While pairing is off no code shows, so no step is the current one.
  assert.deepEqual(steps.pairingSteps('code', true).map(step => step.state), ['next', 'next', 'next']);
  assert.deepEqual(steps.pairingSteps('ready', true).map(step => step.state), ['done', 'done', 'done']);
});

test('one sentence says what to do now, and never shows an address', () => {
  assert.match(steps.pairingLead({ qr: true }), /^Scan the code with Nightjar\./);
  assert.match(steps.pairingLead({ qr: false }), /^Add this PC in Moonlight or another app\./);
  assert.match(steps.pairingLead({ off: true, qr: true }), /^Pairing is off/);
  assert.equal(steps.pairingLead({ pending: ['Steam Deck'] }), 'Steam Deck asks to pair. Type the PIN it shows, if it is the device you expect.');
  assert.match(steps.pairingLead({ pending: ['A', 'B'] }), /^2 devices ask to pair\./);
  assert.equal(steps.pairingLead({ welcome: 'Pixel 9' }), 'Pixel 9 is paired and can play from this PC now.');
  for (const facts of [{}, { qr: true }, { off: true }, { pending: ['A'] }, { welcome: 'A' }]) {
    assert.doesNotMatch(steps.pairingLead(facts), /\d+\.\d+\.\d+|:\d{4,5}\b/);
  }
});

test('the rail marks the current step and reads done steps as done', () => {
  const out = render(Cinema, { props: { facts: { pending: ['Steam Deck'], qr: true }, children: () => {} } }).body;
  assert.match(out, /<ol class="rail[^"]*" aria-label="Pairing steps">/);
  assert.equal((out.match(/aria-current="step"/g) ?? []).length, 1);
  assert.match(out, /aria-current="step"[^>]*>[\s\S]*?Confirm the device/);
  assert.match(out, /Show the code(?:<!--[^>]*-->)*<span class="visually-hidden[^"]*">: done<\/span>/);
  assert.match(out, /Steam Deck asks to pair\./);
  const ready = render(Cinema, { props: { facts: { welcome: 'Pixel 9' }, children: () => {} } }).body;
  assert.doesNotMatch(ready, /aria-current="step"/);
  assert.equal((ready.match(/: done</g) ?? []).length, 3);
});

test('the pairing panel sits in the cinema, and a waiting PIN request leads on one column', () => {
  const panel = source('PairPanel.svelte');
  assert.match(panel, /<PairingCinema facts=\{\{ welcome: welcome\?\.name, pending: session\.pending\.map\(\(request\) => request\.name\), off: pairingOff, qr \}\}>/);
  assert.match(panel, /<div class="ways" class:asking=\{session\.pending\.length > 0 && !pairingOff\}>/);
  assert.match(panel, /@media \(max-width: 900px\) \{[^}]*\} \.asking \.pin \{ order: -1; \} \}/);
  // The PIN's four places wait, empty, where the request will show.
  assert.match(panel, /<span class="slots" aria-hidden="true"><i><\/i><i><\/i><i><\/i><i><\/i><\/span>/);
  // The code keeps its own paper in both themes and shows its time running out.
  const qr = source('QrPairing.svelte');
  assert.match(qr, /background: var\(--qr-paper\);/);
  assert.match(qr, /const LIFETIME = 180;/);
  assert.match(qr, /\{#if code && !expired && !lock\}<span class="time" aria-hidden="true">/);
});

test('while pairing is off no step is current, it is said once, and the code’s place is no white paper', () => {
  const off = render(Cinema, { props: { facts: { off: true, qr: true }, children: () => {} } }).body;
  assert.doesNotMatch(off, /aria-current="step"/);
  assert.doesNotMatch(off, /: done</);
  assert.equal((off.match(/Devices already paired keep working\./g) ?? []).length, 1);
  // The lead says it; the note beside the code's place says what brings a code.
  const panel = source('PairPanel.svelte');
  assert.doesNotMatch(panel, /already paired keep working/);
  assert.match(panel, /lockNote=\{pairingOff \? 'A code shows here once pairing is allowed\.' : 'Pairing with a QR code is off\.'\}/);
  const qr = source('QrPairing.svelte');
  assert.match(qr, /class:locked=\{Boolean\(lock\)\}/);
  assert.match(qr, /\.frame\.locked \{[^}]*background: var\(--sunken\);/);
  assert.match(qr, /<svg class="sample"[^>]*aria-hidden="true">/);
  assert.doesNotMatch(source('sample-qr.ts'), /#fff|data:image/);
});

test('access is a choice of tier cards: one radio group, the recommended one marked', () => {
  const out = render(AccessChoice, { props: { value: 'play', legend: 'This device can', recommend: true, onchange: () => {} } }).body;
  assert.match(out, /<legend[^>]*>This device can<\/legend>/);
  assert.equal((out.match(/type="radio"/g) ?? []).length, 3);
  assert.equal((out.match(/class="choice[^"]*\bon\b/g) ?? []).length, 1);
  assert.match(out, /Play games[\s\S]*?Recommended/);
  for (const label of ['Play games', 'Watch only', 'Everything']) assert.match(out, new RegExp(label));
  const custom = render(AccessChoice, { props: { value: 'custom', legend: 'Access', custom: true, onchange: () => {} } }).body;
  assert.equal((custom.match(/type="radio"/g) ?? []).length, 4);
  assert.doesNotMatch(custom, /Recommended/);
});
