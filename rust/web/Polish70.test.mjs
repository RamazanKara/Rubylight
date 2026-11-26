/**
 * Small findings from the 7.0 visual review, kept from coming back: panel header alignment, the live
 * title's wrap, the table's scroll hint, one codec vocabulary, one set of placeholder initials,
 * the empty log as a card, no search over an empty history, and two spacing and type details.
 */
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { after, before, test } from 'node:test';
import { createTestServer } from './test-server.mjs';

globalThis.location = new URL('http://localhost/');
globalThis.window = new EventTarget();
let server, render, LogView, Capsule, Library, app, shared;
before(async () => {
  server = await createTestServer();
  ({ render } = await server.ssrLoadModule('svelte/server'));
  ({ default: LogView } = await server.ssrLoadModule('/src/components/logs/LogView.svelte'));
  ({ default: Capsule } = await server.ssrLoadModule('/src/play/PlayStatsCapsule.svelte'));
  ({ default: Library } = await server.ssrLoadModule('/src/play/PlayLibrary.svelte'));
  app = await server.ssrLoadModule('/src/components/library/app.ts');
  shared = await server.ssrLoadModule('/src/lib/initials.ts');
});
after(() => server?.close());

const source = (path) => readFileSync(new URL(`./src/${path}`, import.meta.url), 'utf8');

test('a panel’s title and its actions share a centre line, and the live title wraps evenly', () => {
  assert.match(source('components/Panel.svelte'), /\n  header \{[^}]*align-items: center;/);
  assert.match(source('components/overview/LiveCommandStage.svelte'), /\n  h2 \{[^}]*text-wrap: balance;/);
});

test('a table that scrolls sideways fades the edge that hides columns', () => {
  const table = source('components/Table.svelte');
  assert.match(table, /class:more-start=\{more\.start\} class:more-end=\{more\.end\}/);
  assert.match(table, /onscroll=\{measure\}/);
  assert.match(table, /new ResizeObserver\(measure\)/);
  assert.match(table, /\.more-end \{\s*mask-image: linear-gradient\(to left, transparent, var\(--mask-solid\) var\(--space-7\)\);/);
});

test('the stats capsule names the codec as the rest of the page does', () => {
  const capsule = (codec) => render(Capsule, { props: { stats: { fps: 60, mbps: 20, rttMs: 4, codec, width: 1920, height: 1080 }, codec: 'HEVC · 1080p60' } }).body;
  assert.match(capsule('H265'), /HEVC 1080p/);
  assert.doesNotMatch(capsule('H265'), /H265/);
  assert.match(capsule('H264'), /H\.264 1080p/);
  assert.match(capsule('AV1'), /AV1 1080p/);
  // A codec the page does not offer keeps the browser's name.
  assert.match(capsule('VP9'), /VP9 1080p/);
});

test('a game without a cover shows the same letters on the play page and in the console', () => {
  assert.equal(app.initials, shared.initials);
  for (const [title, letters] of [['Portal 2', 'P2'], ['Forza Horizon 5', 'FH'], ['Half-Life 2', 'HL'], ['Hades II', 'H2']]) {
    assert.equal(shared.initials(title), letters);
    const body = render(Library, { props: { apps: [{ id: 'a', title, artworkAvailable: false, running: false }], covers: {}, loading: false, onstart() {} } }).body;
    assert.ok(body.includes(`>${letters}<`), `${title}: ${letters}`);
  }
  assert.doesNotMatch(source('play/PlayLibrary.svelte'), /function initials/);
});

test('an empty log is the icon card, and an empty history has no search field', () => {
  const view = (extra) => render(LogView, { props: { lines: [], pattern: null, matched: new Set(), current: null, placeholder: '', ...extra } }).body;
  const empty = view({ empty: { title: 'The log is empty', text: 'Messages from the host show here as it writes them.' } });
  assert.match(empty, /<div class="empty[^"]*">[\s\S]*<h3[^>]*>The log is empty<\/h3>[\s\S]*Messages from the host show here/);
  assert.match(view({ placeholder: 'Loading the log…' }), /<p class="placeholder[^"]*">Loading the log…<\/p>/);
  const logs = source('pages/Logs.svelte');
  assert.match(logs, /title: 'The log is empty'/);
  assert.match(logs, /<LogView lines=\{visible\} \{pattern\} \{matched\} \{current\} \{placeholder\} \{empty\} bind:follow \/>/);
  assert.match(source('pages/Sessions.svelte'), /\{#if history === null \|\| history\.length > 0\}\s*<div class="toolbar">/);
});

test('the support bundle’s download sits a step under its toggle, and Display mode’s placeholder is not monospace', () => {
  assert.match(source('components/help/SupportBundlePanel.svelte'), /<div class="stack">\s*<Toggle[\s\S]*<div class="row">/);
  assert.match(source('components/devices/DeviceDisplayOverrides.svelte'), /\.input\.mono::placeholder \{\s*font-family: var\(--font\);/);
});
