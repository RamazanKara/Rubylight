import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

// Guests from paired devices on Home (docs/multiclient.md): the card and its helpers.
globalThis.location = new URL('http://localhost/');
globalThis.window = new EventTarget();
const server = await createTestServer();
const guests = await server.ssrLoadModule('/src/lib/session-guests.ts');
const { default: GuestsCard } = await server.ssrLoadModule('/src/components/overview/GuestsCard.svelte');
const { default: StreamCard } = await server.ssrLoadModule('/src/components/overview/StreamCard.svelte');
const { render } = await server.ssrLoadModule('svelte/server');

const status = (changes = {}) => ({
  policy: 'ask',
  default_permission: 'controller',
  owner: 'owner-uuid',
  owner_name: 'Living room PC',
  max_guests: 4,
  version: 1,
  keyboard_mouse: null,
  guests: [],
  waiting: [],
  ...changes,
});
const phone = { id: 2, name: 'Phone', permission: 'controller-keyboard-mouse', streaming: true, since: 1, expires: null };

test('the card shows only with guests or joins waiting', () => {
  assert.equal(guests.shown(null), false);
  assert.equal(guests.shown(status()), false);
  assert.equal(guests.shown(status({ guests: [phone] })), true);
  assert.equal(guests.shown(status({ waiting: [{ id: 3, name: 'Tablet', seconds_left: 40 }] })), true);
});

test('a guest line says what it may do and who has the keyboard and mouse', () => {
  assert.equal(guests.guestLine(status({ guests: [phone] }), 2), 'Controller, keyboard and mouse');
  assert.equal(
    guests.guestLine(status({ guests: [phone], keyboard_mouse: 2 }), 2),
    'Controller, keyboard and mouse · has the keyboard and mouse',
  );
  assert.equal(guests.guestLine(status(), 9), '');
});

test('a stay is until the game ends or a time the host keeps', () => {
  assert.equal(guests.staySeconds('game'), null);
  assert.equal(guests.staySeconds('1h'), 3600);
  assert.equal(guests.staySeconds('24h'), 86_400);
  for (const stay of guests.STAYS) {
    assert.ok(stay.seconds == null || (stay.seconds >= 300 && stay.seconds <= 604_800), stay.value);
  }
  assert.equal(guests.endsIn(null), '');
  assert.equal(guests.endsIn(1000, 1000), 'Ending');
  assert.equal(guests.endsIn(1000 + 59 * 60, 1000), 'Ends in 59 min');
  assert.equal(guests.endsIn(1000 + 5 * 3600, 1000), 'Ends in 5 h');
});

test('the card lists waiting joins with Let in and Deny, and guests with Kick and Block', () => {
  const html = render(GuestsCard, {
    props: {
      status: status({
        guests: [phone, { ...phone, id: 4, name: '<b>TV</b>', permission: 'view', streaming: false }],
        waiting: [{ id: 3, name: 'Tablet', seconds_left: 40 }],
        keyboard_mouse: 2,
      }),
    },
  }).body;
  assert.match(html, /Playing on Living room PC · 2 of 4 guests/);
  assert.match(html, /Waiting to join/);
  assert.match(html, /Tablet/);
  assert.match(html, /40 s left/);
  assert.match(html, /Let in/);
  assert.match(html, /Deny/);
  assert.match(html, /Kick/);
  assert.match(html, /Block/);
  assert.match(html, /Take back keyboard/);
  assert.match(html, /Away/);
  // Names are text, never markup.
  assert.match(html, /&lt;b>TV&lt;\/b>|&lt;b&gt;TV&lt;\/b&gt;/);
  assert.doesNotMatch(html, /<b>TV<\/b>/);
  // Nothing says the feature is unfinished.
  assert.doesNotMatch(html, /experimental|beta|preview/i);
});

test('a guest stream is marked on its stream card', () => {
  const stream = {
    device_name: 'Phone', state: 'RUNNING', width: 1280, height: 720, fps: 60,
    video_format: 1, encoder: 'amf', encoder_bitrate_kbps: 20_000,
    pyrowave_minimum_kbps: null, pyrowave_recommended_kbps: null,
    uptime_seconds: 1, performance: { sample_frames: 0, history: [] }, warnings: [],
  };
  const card = (changes) => render(StreamCard, { props: { stream: { ...stream, ...changes }, ondisconnect: () => {} } }).body;
  assert.match(card({ guest: 2 }), />Guest</);
  assert.doesNotMatch(card({ guest: null }), />Guest</);
  assert.doesNotMatch(card({}), />Guest</);
});
