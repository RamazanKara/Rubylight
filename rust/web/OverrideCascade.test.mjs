import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { after, before, test } from 'node:test';
import { createTestServer } from './test-server.mjs';

// Rubylight 7.0's override cascade (#140): override-cascade.ts resolves, OverrideCascadeCards draws,
// the game page and the device page write their own layer. Also the game page's cover-lit header.
globalThis.location = new URL('http://localhost/');
globalThis.window = new EventTarget();
let server, render, lib, Cards, Header, studio, allowed;
before(async () => {
  server = await createTestServer();
  ({ render } = await server.ssrLoadModule('svelte/server'));
  lib = await server.ssrLoadModule('/src/components/library/override-cascade.ts');
  ({ default: Cards } = await server.ssrLoadModule('/src/components/library/OverrideCascadeCards.svelte'));
  ({ default: Header } = await server.ssrLoadModule('/src/components/library/GameStudioHeader.svelte'));
  studio = await server.ssrLoadModule('/src/components/library/game-studio.ts');
  ({ OVERRIDE_ALLOWED: allowed } = await server.ssrLoadModule('/src/lib/schema/overrides.gen.ts'));
});
after(() => server?.close());

const read = path => readFileSync(new URL(path, import.meta.url), 'utf8');
const source = name => read(`./src/components/${name}`);
const layers = ({ pair, app, device, host = {} } = {}) => [
  { id: 'pair', name: 'This game, this device', values: pair },
  { id: 'app', name: 'This game', values: app },
  { id: 'device', name: 'Living room TV', values: device },
  { id: 'host', name: 'Host', values: host },
];

test('every card is a setting the catalog lets a layer override, and the #140 keys are all there', () => {
  const catalog = read('../core/config/settings.toml');
  assert.ok(allowed.length > 100);
  for (const key of lib.CASCADE_KEYS) {
    assert.ok(allowed.includes(key), `${key} is override_allowed`);
    assert.ok(lib.cascadeSetting(key), `${key} has a catalog entry`);
    assert.match(catalog, new RegExp(`key = "${key}"`));
  }
  assert.equal(new Set(lib.CASCADE_KEYS).size, lib.CASCADE_KEYS.length);
  assert.deepEqual(lib.CASCADE_GROUPS.map(group => group.title), ['Display', 'Stream and codec', 'Sound', 'Input']);
  for (const key of ['dd_resolution_option', 'dd_refresh_rate_option', 'dd_hdr_option', 'virtual_display_refresh', 'dd_activate_virtual_display',
    'max_bitrate', 'fec_percentage', 'hevc_mode', 'av1_mode', 'packed_444', 'frame_limiter_enable', 'frame_limiter_provider', 'frame_limiter_fps_limit',
    'virtual_sink', 'audio_capture_mode', 'stream_mic', 'audio_surround_714', 'mouse_mode', 'controller']) {
    assert.ok(lib.CASCADE_KEYS.includes(key), key);
  }
  // A host-wide key gets no card, whatever a page asks for.
  assert.equal(lib.cascadeSetting('sunshine_name'), undefined);
  assert.equal(allowed.includes('sunshine_name'), false);
});

test('the deciding layer is the first that sets a value: game on device, game, device, then the host', () => {
  const bitrate = lib.cascadeSetting('max_bitrate');
  const owner = values => lib.resolve(bitrate, layers(values)).owner;
  assert.equal(owner({ pair: { max_bitrate: 80000 }, app: { max_bitrate: 60000 }, device: { max_bitrate: 40000 } }), 'pair');
  assert.equal(owner({ app: { max_bitrate: 60000 }, device: { max_bitrate: 40000 } }), 'app');
  assert.equal(owner({ device: { max_bitrate: 40000 } }), 'device');
  assert.equal(owner({}), 'host');
  // As the host merges: an empty or null override is not one.
  assert.equal(owner({ pair: { max_bitrate: '' }, app: { max_bitrate: null }, device: { max_bitrate: 40000 } }), 'device');
  const resolved = lib.resolve(bitrate, layers({ pair: { max_bitrate: 80000 }, device: { max_bitrate: 40000 }, host: { max_bitrate: 20000 } }));
  assert.equal(resolved.value, 80000);
  assert.deepEqual(resolved.steps.map(step => [step.layer.id, step.set, step.wins]),
    [['pair', true, true], ['app', false, false], ['device', true, false], ['host', true, false]]);
  // The host always has a value: its own, else the catalog's default.
  assert.equal(lib.resolve(bitrate, layers({})).value, bitrate.default);
  assert.equal(lib.resolve(bitrate, layers({ host: null })).unknown, true);
  assert.equal(lib.resolve(bitrate, layers({ device: { max_bitrate: 1 }, host: null })).unknown, false);
  assert.deepEqual(lib.OWNER_TAG, { pair: 'Custom for this device', app: 'App override', device: 'From Device', host: 'Inherited from Host' });
});

test('values read in words, and a control stores what the host reads', () => {
  const hdr = lib.cascadeSetting('dd_hdr_option');
  const chroma = lib.cascadeSetting('packed_444');
  const bitrate = lib.cascadeSetting('max_bitrate');
  const fec = lib.cascadeSetting('fec_percentage');
  assert.equal(lib.shown(hdr, 'force_off'), 'Always off');
  assert.equal(lib.shown(chroma, true), 'On');
  assert.equal(lib.shown(chroma, 'false'), 'Off');
  assert.equal(lib.shown(bitrate, 40000), '40000 Kbps');
  assert.equal(lib.shown(fec, 10), '10%');
  assert.equal(lib.shown(lib.cascadeSetting('hevc_mode'), 2), 'SDR only');
  assert.equal(lib.shown(lib.cascadeSetting('dd_manual_resolution'), ''), 'Not set');
  assert.equal(lib.controlText(chroma, false), 'off');
  assert.equal(lib.controlText(chroma, undefined), '');
  assert.equal(lib.controlText(bitrate, 40000), '40000');
  assert.equal(lib.storedValue(chroma, 'on'), true);
  assert.equal(lib.storedValue(chroma, 'off'), false);
  assert.equal(lib.storedValue(bitrate, ' 40000 '), 40000);
  assert.equal(lib.storedValue(hdr, 'force_on'), 'force_on');
  assert.equal(lib.storedValue(hdr, ''), undefined, 'empty clears the layer');
  assert.equal(lib.rangeError(fec, '250'), 'At most 100%.');
  assert.equal(lib.rangeError(fec, '-1'), 'At least 0%.');
  assert.equal(lib.rangeError(fec, 'abc'), 'Enter a number.');
  assert.equal(lib.rangeError(fec, ''), '');
  assert.equal(lib.setCount(['max_bitrate', 'hevc_mode', 'av1_mode'], { max_bitrate: 1, hevc_mode: '', av1_mode: '0' }), 2);
});

test('a card names who decides, shows every layer, and offers Reset to inherited only for its own value', () => {
  const out = render(Cards, { props: {
    id: 'c',
    layers: layers({ pair: { max_bitrate: 80000 }, app: { fec_percentage: 10 }, device: { max_bitrate: 40000, dd_hdr_option: 'force_off' }, host: { fec_percentage: 20 } }),
    groups: [{ id: 'stream', title: 'Stream and codec', keys: ['max_bitrate', 'fec_percentage', 'dd_hdr_option', 'packed_444'] }],
    onset: () => {},
  } }).body;
  assert.equal((out.match(/<article class="card[^"]*"/g) ?? []).length, 4);
  for (const tag of ['Custom for this device', 'App override', 'From Device', 'Inherited from Host']) assert.match(out, new RegExp(tag));
  assert.equal((out.match(/Reset to inherited/g) ?? []).length, 1);
  assert.equal((out.match(/class="[^"]*\bwins\b/g) ?? []).length, 4, 'one deciding layer per card');
  assert.equal((out.match(/<li[^>]*>/g) ?? []).length, 16, 'four layers on each card');
  assert.match(out, /80000 Kbps/);
  assert.match(out, /Always off/);
  // The control of this layer: empty follows the next layer, named with what it would give.
  assert.match(out, /<option value=""[^>]*>Inherit: 40000 Kbps<\/option>|placeholder="Inherit: 40000 Kbps"/);
  assert.match(out, /<label class="visually-hidden[^"]*" for="c-max_bitrate">Maximum bitrate for This game, this device<\/label>/);
  assert.match(out, /Stream and codec[\s\S]*?4 settings, 1 set here/);
});

test('the cards skip what a page already offers, and say so when the host cannot be read', () => {
  const props = { id: 'c', layers: layers({ host: null }), onset: () => {} };
  const all = render(Cards, { props }).body;
  assert.equal((all.match(/<article class="card/g) ?? []).length, lib.CASCADE_KEYS.length);
  assert.match(all, /Reading…/);
  const skipped = render(Cards, { props: { ...props, skip: ['frame_limiter_fps_limit'] } }).body;
  assert.equal((skipped.match(/<article class="card/g) ?? []).length, lib.CASCADE_KEYS.length - 1);
  const failed = render(Cards, { props: { ...props, hostError: 'timeout' } }).body;
  assert.match(failed, /The host’s settings could not be read/);
  assert.match(failed, /Overrides can still be set\./);
  assert.doesNotMatch(failed, /Reading…/);
});

test('each page writes only its own layer, through its own save', () => {
  const game = source('library/DeviceOverrides.svelte');
  assert.match(game, /onset=\{\(key, value\) => draft\.setDeviceSetting\(device, key, value\)\}/);
  assert.match(game, /skip=\{\[FRAME_LIMIT\]\}/);
  assert.match(game, /\{ id: 'pair', name: 'This game, this device',[^}]*\},\s*\{ id: 'app', name: 'This game',[^}]*\},\s*\{ id: 'device',[^}]*\},\s*\{ id: 'host', name: 'Host', values: host \}/);
  // What the cards show is no longer listed as "also kept".
  assert.match(game, /!CASCADE_KEYS\.includes\(key\)/);
  const device = source('devices/DeviceEditor.svelte');
  assert.match(device, /\{ id: 'device', name: draft\.name\.trim\(\) \|\| 'This device', values: draft\.other_overrides \},\s*\{ id: 'host', name: 'Host', values: hostConfig \}/);
  assert.match(device, /if \(value === undefined\) delete draft\.other_overrides\[key\];\s*else draft\.other_overrides\[key\] = value;/);
  // No write of their own: the cards call back and nothing else.
  assert.doesNotMatch(source('library/OverrideCascadeCards.svelte'), /api\.|fetch\(/);
});

test('the game page opens with a cover-lit header: launcher, name, state, stream profile and quick actions', () => {
  const facts = studio.studioFacts({ name: 'Portal 2', 'device-overrides': { tv: {}, deck: {} }, 'capture-window': 'window', 'not-installed': true });
  assert.deepEqual(facts.map(fact => fact.text), ['Not installed', 'Streams its window', '2 device profiles']);
  assert.equal(facts[0].tone, 'warn');
  assert.deepEqual(studio.studioFacts({ name: 'Desktop' }).map(fact => fact.text), ['Host stream settings']);
  assert.deepEqual(studio.studioFacts({ name: 'A', 'device-overrides': { tv: {} } }).map(fact => fact.text), ['1 device profile']);
  const out = render(Header, { props: { name: 'Portal 2', uuid: 'steam-620', launcher: 'Steam', status: 'running', facts } }).body;
  assert.match(out, /<h1[^>]*>Portal 2<\/h1>/);
  assert.match(out, /Steam/);
  assert.match(out, /Running/);
  assert.match(out, /<ul class="facts[^"]*" aria-label="Stream profile">/);
  // The art first, its wash after it and hidden: the router morphs the first cover it finds.
  assert.ok(out.indexOf('class="art') < out.indexOf('class="wash'));
  assert.match(out, /<div class="wash[^"]*" aria-hidden="true">/);
  assert.match(render(Header, { props: { name: 'Notepad' } }).body, /Your own app/);
  const page = source('library/AppEditor.svelte');
  assert.match(page, /\{#if isNew\}\s*<PageHeader title="New app"/);
  assert.match(page, /<GameStudioHeader[\s\S]*?facts=\{studioFacts\(draft\.saved\)\}/);
  assert.match(page, /<Button icon="image" onclick=\{editCover\}>Edit cover<\/Button>/);
  assert.match(page, /<Panel id="cover" title="Cover">/);
  assert.doesNotMatch(source('library/GameStudioHeader.svelte'), /backdrop-filter/);
});
