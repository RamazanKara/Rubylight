import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { END, START, applyToDocs, cell, keyLabel, renderBlock, rowFor, socName, sortedEntries, vendorKeys } from './device-docs.mjs';

const QTI_KEYS = ['vendor.qti-ext-dec-picture-order.enable', 'vendor.qti-ext-output-fence.enable', 'vendor.qti-ext-output-fence.fence_type'];

function entry(issue, { name = 'Phone', soc = ['QTI', 'SM8750'], hevc = 1, h264 = 2, av1 = null, keys = QTI_KEYS, hz = 120, version = '0.8.0' } = {}) {
  const decoder = (codec, medianDecodeMs, vendorLowLatencyKeys) => ({ codec, medianDecodeMs, vendorLowLatencyKeys });
  const results = [];
  if (h264 !== null) results.push(decoder('H.264', h264, keys));
  if (hevc !== null) results.push(decoder('HEVC', hevc, keys));
  if (av1 !== null) results.push(decoder('AV1', av1, keys));
  return {
    issue,
    added: '2026-10-20',
    name,
    results: {
      app: { name: 'Nightjar', version, build: 1 },
      device: { manufacturer: 'acme', model: 'X1', socManufacturer: soc[0], socModel: soc[1], hardware: 'qcom', android: '16' },
      decoderBenchmark: { measured: true, clip: '1920x1080@60', results },
      latencyTest: hz === null ? null : { panelHz: hz }
    }
  };
}

const docs = 'intro\n\n' + START + '\nstale\n' + END + '\n\n| To measure |\n| --- |\n| hand written |\n';

test('a row has the device, SoC, times, shortened keys and a linked note', () => {
  const row = rowFor(entry(11, { name: 'Samsung Galaxy S25 Ultra', h264: 0.6, hevc: 0.55, av1: 0.39, version: '0.7.13' }));
  assert.equal(row, '| Samsung Galaxy S25 Ultra | Snapdragon 8 Elite (SM8750) | 16 | 0.60 | 0.55 | 0.39 | picture order, output fence | Nightjar 0.7.13, 120 Hz. [#11](https://github.com/RamazanKara/nightjar/issues/11) |');
});

test('a maintainer note goes between the app version and the issue link', () => {
  const noted = entry(26, { version: '3.0.0' });
  noted.note = 'Retest pending.';
  assert.match(rowFor(noted), /Nightjar 3\.0\.0, 120 Hz\. Retest pending\. \[#26\]\(/);
});

test('missing codecs show a dash and a missing latency test is said so', () => {
  const row = rowFor(entry(23, { hevc: null, h264: 4.48, hz: null }));
  assert.match(row, / \| 4\.48 \| — \| — \| /);
  assert.match(row, /Nightjar 0\.8\.0; no latency test\. \[#23\]/);
  const unmeasured = entry(30);
  unmeasured.results.decoderBenchmark.measured = false;
  assert.match(rowFor(unmeasured), / \| — \| — \| — \| — \| /);
});

test('SoC names use the model number for chips without a known name', () => {
  assert.equal(socName({ socManufacturer: 'Mediatek', socModel: 'MT6899' }), 'MediaTek MT6899');
  assert.equal(socName({ socManufacturer: 'Samsung', socModel: 's5e8845' }), 'Samsung s5e8845');
  assert.equal(socName({ socManufacturer: 'QTI', socModel: 'SM9999' }), 'Qualcomm SM9999');
  assert.equal(socName({ socManufacturer: 'QTI', socModel: 'sm8550' }), 'Snapdragon 8 Gen 2 (sm8550)');
  assert.equal(socName({ socModel: null, hardware: 'amlogic' }), 'amlogic');
  assert.equal(socName({}), '—');
});

test('vendor keys are shortened and codec-specific keys are marked', () => {
  assert.equal(keyLabel('vendor.mtk.vdec.cpu.boost.mode.value'), 'CPU boost');
  assert.equal(keyLabel('vendor.rtc-ext-dec-low-latency.enable'), '`rtc-ext-dec-low-latency`');
  const decoder = (codec, keys) => ({ codec, vendorLowLatencyKeys: keys });
  assert.equal(vendorKeys([decoder('H.264', ['vendor.rtc-ext-dec-low-latency.enable']), decoder('HEVC', [])]), '`rtc-ext-dec-low-latency` (H.264 only)');
  assert.equal(vendorKeys([decoder('H.264', ['vendor.a.enable']), decoder('HEVC', ['vendor.a.enable']), decoder('AV1', ['vendor.b'])]), '`a` (H.264, HEVC), `b` (AV1 only)');
  assert.equal(vendorKeys([decoder('H.264', ['vendor.a', 'vendor.b']), decoder('HEVC', ['vendor.a'])]), '`a`, `b` (H.264 only)');
  assert.equal(vendorKeys([decoder('H.264', []), decoder('HEVC', [])]), 'None');
  assert.equal(vendorKeys([decoder('H.264', null)]), '—');
  assert.equal(vendorKeys([]), '—');
});

test('table text cannot break out of its cell', () => {
  assert.equal(cell('a|b [c] *d* _e_'), 'a\\|b \\[c\\] \\*d\\* \\_e\\_');
  assert.ok(!rowFor(entry(1, { name: 'x | y' })).includes('x | y'));
});

test('entries are sorted by fastest HEVC, devices without HEVC last, then by issue', () => {
  const table = { entries: [entry(5, { hevc: 4 }), entry(6, { hevc: null }), entry(3, { hevc: 1 }), entry(4, { hevc: 1 }), entry(2, { hevc: null })] };
  assert.deepEqual(sortedEntries(table).map(item => item.issue), [3, 4, 5, 2, 6]);
});

test('the block replaces only what is between the markers and regenerating is idempotent', () => {
  const table = { entries: [entry(5, { hevc: 4 }), entry(3, { hevc: 1 })] };
  const once = applyToDocs(docs, table);
  assert.ok(once.startsWith('intro\n\n' + START + '\n'));
  assert.ok(once.endsWith(END + '\n\n| To measure |\n| --- |\n| hand written |\n'));
  assert.ok(once.includes(renderBlock(table)));
  assert.equal(once.split('\n').filter(line => line.startsWith('| Phone')).length, 2);
  assert.ok(!once.includes('stale'));
  assert.equal(applyToDocs(once, table), once);
  assert.ok(once.indexOf('[#3]') < once.indexOf('[#5]'));
});

test('docs without the markers are refused', () => {
  assert.throws(() => applyToDocs('no markers here', { entries: [] }), /markers/);
  assert.throws(() => applyToDocs(END + '\n' + START, { entries: [] }), /markers/);
});

test('docs/devices.md matches site/data/device-results.json', () => {
  const read = path => readFileSync(new URL(path, import.meta.url), 'utf8');
  const current = read('../docs/devices.md');
  assert.equal(applyToDocs(current, JSON.parse(read('../site/data/device-results.json'))), current, 'run: node scripts/device-docs.mjs');
});
