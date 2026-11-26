import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { after, before, test } from 'node:test';
import { createTestServer } from './test-server.mjs';

// Rubylight 7.0's display map: display-topology.ts lays it out, DisplayTopologyCanvas draws it, read-only.
globalThis.location = new URL('http://localhost/');
globalThis.window = new EventTarget();
let server, render, lib, Canvas;
before(async () => {
  server = await createTestServer();
  ({ render } = await server.ssrLoadModule('svelte/server'));
  lib = await server.ssrLoadModule('/src/components/devices/display-topology.ts');
  ({ default: Canvas } = await server.ssrLoadModule('/src/components/devices/DisplayTopologyCanvas.svelte'));
});
after(() => server?.close());

const source = name => readFileSync(new URL(`./src/components/${name}`, import.meta.url), 'utf8');
const display = (id, name, extra = {}) => ({
  device_id: `{${id}}`, display_name: `\\\\.\\DISPLAY${id}`, friendly_name: name, hdr_supported: false, hdr_enabled: false, primary: false, ...extra,
});
const desk = [
  display(1, 'Studio', { primary: true, hdr_supported: true, hdr_enabled: true, x: 0, y: 0, width: 3840, height: 2160, refresh: 144, scale: 150, virtual: false }),
  display(2, 'Side', { x: -1200, y: -220, width: 1200, height: 1920, refresh: 60, scale: 100, virtual: false }),
  display(3, 'Rubylight Virtual Display', { hdr_supported: true, x: 3840, y: 0, width: 2560, height: 1600, refresh: 120, scale: 200, virtual: true }),
];
const near = (a, b) => Math.abs(a - b) < 0.01;

test('each display is drawn to scale where Windows puts it, negative places included', () => {
  const map = lib.topology(desk);
  assert.equal(map.spatial, true);
  // The desktop spans x -1200..6400 and y -220..2160.
  assert.ok(near(map.ratio, 7600 / 2380));
  const [studio, side, virtual] = map.tiles;
  assert.ok(near(side.left, 0) && near(side.top, 0));
  assert.ok(near(studio.left, (1200 / 7600) * 100) && near(studio.top, (220 / 2380) * 100));
  assert.ok(near(studio.width, (3840 / 7600) * 100) && near(studio.height, (2160 / 2380) * 100));
  assert.ok(near(virtual.left + virtual.width, 100));
  for (const tile of map.tiles) {
    assert.ok(tile.left >= 0 && tile.top >= 0 && tile.left + tile.width <= 100.01 && tile.top + tile.height <= 100.01, tile.name);
  }
  assert.deepEqual(map.tiles.map(tile => tile.index), [1, 2, 3]);
});

test('a tile carries the mode, the scale, HDR, its kind and its place from the primary display', () => {
  const [studio, side, virtual] = lib.topology(desk).tiles;
  assert.equal(studio.mode, '3840 × 2160 · 144 Hz');
  assert.equal(studio.scale, '150%');
  assert.equal(studio.hdr, 'on');
  assert.equal(studio.place, 'Primary display');
  assert.equal(side.hdr, null, 'no HDR badge for a display without HDR');
  assert.equal(side.place, 'Left of Studio');
  assert.equal(virtual.hdr, 'off');
  assert.equal(virtual.virtual, true);
  assert.equal(virtual.place, 'Right of Studio');
  assert.equal(lib.sideOf({ x: 0, y: -1080, width: 1920, height: 1080 }, { x: 0, y: 0, width: 1920, height: 1080 }), 'Above');
  assert.equal(lib.sideOf({ x: 0, y: 1080, width: 1920, height: 1080 }, { x: 0, y: 0, width: 1920, height: 1080 }), 'Below');
  assert.equal(lib.modeText({ width: 1920, height: 1080 }), '1920 × 1080');
  assert.equal(lib.describe(virtual), 'Rubylight Virtual Display, 2560 × 1600 at 120 Hz, 200% scale, HDR off, virtual display, right of studio');
});

test('the display a stream uses is matched as the host matches it: by id or by Windows name, any case', () => {
  assert.equal(lib.topology(desk, '{2}').tiles.filter(tile => tile.target).map(tile => tile.name).join(), 'Side');
  assert.equal(lib.topology(desk, '\\\\.\\display3').tiles.filter(tile => tile.target).map(tile => tile.name).join(), 'Rubylight Virtual Display');
  assert.equal(lib.topology(desk, '').tiles.some(tile => tile.target), false);
  assert.equal(lib.topology(desk, 'Studio').tiles.some(tile => tile.target), false, 'a friendly name is not an id');
});

test('an older host that reports no places still lists every display, in a row and without a place', () => {
  const bare = [display(1, 'Studio', { primary: true }), display(2, '')];
  const map = lib.topology(bare);
  assert.equal(map.spatial, false);
  assert.deepEqual(map.tiles.map(tile => tile.name), ['Studio', 'Display 2']);
  assert.deepEqual(map.tiles.map(tile => tile.mode), ['', '']);
  assert.ok(near(map.tiles[0].width, 50) && near(map.tiles[1].left, 50));
  assert.equal(map.tiles[1].place, '');
  // One display without a place makes the whole map a row: a half-placed map would lie.
  assert.equal(lib.topology([desk[0], display(2, 'Side')]).spatial, false);
  assert.deepEqual(lib.topology([]), { spatial: false, ratio: 1, tiles: [] });
});

test('the canvas draws the map for the eye and the list for everyone, in each of its states', () => {
  const out = render(Canvas, { props: { displays: desk, target: '{1}', targetLabel: 'This device streams here' } }).body;
  assert.match(out, /<div class="map[^"]*"[^>]*aria-hidden="true"/);
  assert.equal((out.match(/class="tile[^"]*"/g) ?? []).length, 3);
  assert.match(out, /<ul class="list[^"]*" aria-label="Displays">/);
  assert.match(out, /3840 × 2160 · 144 Hz/);
  assert.match(out, /This device streams here/);
  for (const pill of ['Primary ★', '150%', 'HDR on', 'HDR off', 'Virtual', 'Physical']) assert.match(out, new RegExp(pill.replace('★', '\\u2605')));
  assert.match(out, /Studio, 3840 × 2160 at 144 Hz, 150% scale, HDR on, primary display, streams to this device/);
  assert.match(out, /To move one, use Windows’ display settings\./);
  // Loading, empty, an error with and without stale data, and no places.
  assert.match(render(Canvas, { props: { displays: null } }).body, /Loading the displays/);
  assert.match(render(Canvas, { props: { displays: [] } }).body, /No display is connected/);
  assert.match(render(Canvas, { props: { displays: null, error: 'access denied' } }).body, /The displays could not be read: access denied/);
  assert.match(render(Canvas, { props: { displays: desk, error: 'timeout' } }).body, /This may be out of date[^<]*timeout/);
  assert.match(render(Canvas, { props: { displays: [display(1, 'Studio')] } }).body, /Windows did not report where these displays sit/);
});

test('the map is read-only: it has no control, and the pages that show it keep their own reviewed writes', () => {
  const canvas = source('devices/DisplayTopologyCanvas.svelte');
  assert.doesNotMatch(canvas, /<button|<input|<select|draggable|onclick|api\./);
  const panel = source('maintenance/DisplaysPanel.svelte');
  assert.match(panel, /<DisplayTopologyCanvas \{displays\} error=\{displaysError\} onretry=\{loadDisplays\} \/>/);
  // Restoring and resetting still ask first, as before.
  assert.match(panel, /title: 'Reset display settings memory\?'/);
  const device = source('devices/DeviceDisplayOverrides.svelte');
  assert.match(device, /<DisplayTopologyCanvas \{displays\} target=\{draft\.output_name_override\} targetLabel="This device streams here" \/>/);
});
