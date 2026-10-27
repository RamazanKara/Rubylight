import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve, sep } from 'node:path';
import test from 'node:test';
import { addEntry, emptyTable, main, parseResults, replaceEntry, validateResults } from './add-device-results.mjs';
import { END, START } from './device-docs.mjs';

// The layout DeviceResultsTest pins for the app's export.
const sample = `{
  "format": "nightjar-device-results",
  "version": 1,
  "app": {
    "name": "Nightjar",
    "version": "0.6.0",
    "build": 331
  },
  "device": {
    "manufacturer": "samsung",
    "model": "SM-S918B",
    "codename": "dm3q",
    "socManufacturer": "QTI",
    "socModel": "SM8550",
    "hardware": "qcom",
    "board": "kalama",
    "android": "14",
    "sdk": 34,
    "gpu": "Adreno (TM) 740"
  },
  "decoderBenchmark": {
    "measured": true,
    "clip": "1920x1080@60",
    "results": [
      {
        "codec": "H.264",
        "mime": "video/avc",
        "decoder": "c2.qti.avc.decoder",
        "medianDecodeMs": 2.10,
        "featureLowLatency": true,
        "vendorLowLatencyKeys": [
          "vendor.qti-ext-dec-low-latency.enable",
          "vendor.qti-ext-dec-picture-order.enable"
        ]
      },
      {
        "codec": "HEVC",
        "mime": "video/hevc",
        "decoder": "c2.qti.hevc.decoder",
        "medianDecodeMs": 2.45,
        "featureLowLatency": null,
        "vendorLowLatencyKeys": null
      }
    ]
  },
  "latencyTest": {
    "panelHz": 120.00,
    "audioSampleRate": 48000,
    "audioFramesPerBuffer": 192,
    "frameIntervalMs": {
      "samples": 1199,
      "mean": 8.33,
      "p95": 8.40,
      "p99": 9.13
    },
    "frameCallbackDelayMs": null,
    "inputDispatchAgeMs": null,
    "timerWakeupDelayMs": null
  }
}`;

const parsed = () => JSON.parse(sample);

test('the app export passes and keeps the format order', () => {
  const { errors, dropped, results } = validateResults(parsed());
  assert.deepEqual(errors, []);
  assert.deepEqual(dropped, []);
  assert.equal(JSON.stringify(results, null, 2), JSON.stringify(parsed(), null, 2));
});

test('an issue body with a code block around the text parses', () => {
  const body = '### Results text\n\n```json\n' + sample + '\n```\n\n### Test conditions\n\nPlugged in';
  assert.deepEqual(parseResults(body), parsed());
  assert.throws(() => parseResults('no results here'), /no JSON object/);
});

test('other formats and versions are rejected', () => {
  const other = { ...parsed(), format: 'something-else', version: 2 };
  const { errors, results } = validateResults(other);
  assert.equal(results, null);
  assert.ok(errors.some(error => error.startsWith('format:')));
  assert.ok(errors.some(error => error.startsWith('version:')));
});

test('unknown fields are dropped instead of published', () => {
  const input = parsed();
  input.serial = 'R5CT1234567';
  input.device.fingerprint = 'samsung/dm3q/...';
  input.decoderBenchmark.results[0].note = 'extra';
  const { errors, dropped, results } = validateResults(input);
  assert.deepEqual(errors, []);
  assert.deepEqual(dropped, ['serial', 'device.fingerprint', 'decoderBenchmark.results[0].note']);
  assert.ok(!JSON.stringify(results).includes('R5CT'));
  assert.ok(!('fingerprint' in results.device));
});

test('addresses, accounts, tokens and markup are refused', () => {
  const input = parsed();
  input.device.model = 'Box 192.168.1.20';
  input.device.manufacturer = 'owner@example.com';
  input.device.gpu = 'Renderer 0123456789abcdef0123';
  input.device.codename = 'fe80::1:2:3';
  input.device.board = '<script>';
  input.decoderBenchmark.results[0].decoder = 'c2 decoder with spaces';
  input.decoderBenchmark.results[0].vendorLowLatencyKeys = ['not-vendor'];
  const { errors, results } = validateResults(input);
  assert.equal(results, null);
  for (const field of ['device.model', 'device.manufacturer', 'device.gpu', 'device.codename', 'device.board',
    'decoderBenchmark.results[0].decoder', 'decoderBenchmark.results[0].vendorLowLatencyKeys']) {
    assert.ok(errors.some(error => error.startsWith(field + ':')), field);
  }
});

test('placeholders from the app redaction are allowed', () => {
  const input = parsed();
  input.device.model = 'Box [address] of [PC]';
  input.decoderBenchmark.results[0].decoder = '[omitted]';
  assert.deepEqual(validateResults(input).errors, []);
});

test('numbers must be in range', () => {
  const input = parsed();
  input.decoderBenchmark.results[0].medianDecodeMs = -1;
  input.device.sdk = 'thirty four';
  input.latencyTest.frameIntervalMs.samples = 0;
  const { errors } = validateResults(input);
  assert.equal(errors.length, 3);
});

test('entries need an issue, a clean name and are not added twice', () => {
  const { results } = validateResults(parsed());
  const table = emptyTable();
  addEntry(table, results, { issue: 12, name: 'Galaxy S23', added: '2026-10-15' });
  assert.deepEqual(table.entries[0], { issue: 12, added: '2026-10-15', name: 'Galaxy S23', results });
  assert.throws(() => addEntry(table, results, { issue: 12, added: '2026-10-15' }), /already in the table/);
  assert.throws(() => addEntry(table, results, { issue: 13, added: '2026-10-15' }), /exact results/);
  assert.throws(() => addEntry(emptyTable(), results, { added: '2026-10-15' }), /issue number/);
  assert.throws(() => addEntry(emptyTable(), results, { issue: 14, name: 'mail me@example.com', added: '2026-10-15' }), /--name/);
  assert.throws(() => addEntry({ entries: [] }, results, { issue: 15, added: '2026-10-15' }), /not a nightjar-device-table/);
});

test('the command line appends to the data file and the checked-in file is valid', t => {
  const parent = resolve(tmpdir());
  const root = mkdtempSync(join(parent, 'nightjar-device-results-'));
  t.after(() => {
    assert.ok(resolve(root).startsWith(parent + sep + 'nightjar-device-results-'));
    rmSync(root, { recursive: true });
  });
  const shipped = JSON.parse(readFileSync(new URL('../site/data/device-results.json', import.meta.url), 'utf8'));
  assert.equal(shipped.format, 'nightjar-device-table');
  for (const entry of shipped.entries) assert.deepEqual(validateResults(entry.results).errors, [], 'issue #' + entry.issue);
  writeFileSync(join(root, 'table.json'), JSON.stringify(emptyTable()));
  writeFileSync(join(root, 'pasted.txt'), '```json\n' + sample + '\n```\n');
  main(['pasted.txt', '--issue', '7', '--name', 'Galaxy S23', '--data', 'table.json', '--date', '2026-10-15'], root);
  const table = JSON.parse(readFileSync(join(root, 'table.json'), 'utf8'));
  assert.equal(table.entries.length, 1);
  assert.equal(table.entries[0].results.device.socModel, 'SM8550');
  assert.throws(() => main(['pasted.txt', '--issue', '8', '--data', 'table.json'], root), /exact results/);
});

test('replaceEntry updates an issue in place and keeps the added date', () => {
  const { results } = validateResults(parsed());
  const table = emptyTable();
  assert.equal(replaceEntry(table, results, { issue: 12, name: 'Galaxy S23', added: '2026-10-15' }), 'added');
  assert.equal(replaceEntry(table, results, { issue: 12, name: 'Galaxy S23', added: '2026-11-01' }), 'unchanged');
  assert.equal(replaceEntry(table, results, { issue: 12, added: '2026-11-01' }), 'unchanged');

  const faster = structuredClone(results);
  faster.decoderBenchmark.results[0].medianDecodeMs = 1.5;
  assert.equal(replaceEntry(table, faster, { issue: 12, name: 'Galaxy S23 Plus', added: '2026-11-01' }), 'updated');
  assert.equal(table.entries.length, 1);
  assert.deepEqual(table.entries[0], { issue: 12, added: '2026-10-15', name: 'Galaxy S23 Plus', results: faster });

  // Another issue may not publish the same text, and plain addEntry still refuses a known issue.
  assert.throws(() => replaceEntry(table, faster, { issue: 13, added: '2026-11-01' }), /exact results/);
  assert.throws(() => addEntry(table, faster, { issue: 12, added: '2026-11-01' }), /already in the table/);
  const other = structuredClone(results);
  other.device.model = 'SM-X';
  replaceEntry(table, other, { issue: 13, added: '2026-11-02' });
  assert.throws(() => replaceEntry(table, other, { issue: 12, added: '2026-11-03' }), /under another issue/);
});

test('--replace and --docs update the table data and the docs together', t => {
  const parent = resolve(tmpdir());
  const root = mkdtempSync(join(parent, 'nightjar-device-results-'));
  t.after(() => {
    assert.ok(resolve(root).startsWith(parent + sep + 'nightjar-device-results-'));
    rmSync(root, { recursive: true });
  });
  writeFileSync(join(root, 'table.json'), JSON.stringify(emptyTable()));
  writeFileSync(join(root, 'devices.md'), '# Devices\n\n' + START + '\nold\n' + END + '\n\n| To measure |\n');
  writeFileSync(join(root, 'pasted.txt'), sample);
  const args = ['pasted.txt', '--issue', '7', '--name', 'Galaxy S23', '--data', 'table.json', '--docs', 'devices.md', '--replace'];
  main([...args, '--date', '2026-10-15'], root);
  const docs = readFileSync(join(root, 'devices.md'), 'utf8');
  assert.match(docs, /\| Galaxy S23 \| Snapdragon 8 Gen 2 \(SM8550\) \| 14 \| 2\.10 \| 2\.45 \| — \|/);
  assert.ok(docs.endsWith('\n\n| To measure |\n'));
  // The same issue again replaces instead of failing, and nothing changes when the text is identical.
  main([...args, '--date', '2026-12-01'], root);
  assert.equal(readFileSync(join(root, 'devices.md'), 'utf8'), docs);
  assert.equal(JSON.parse(readFileSync(join(root, 'table.json'), 'utf8')).entries[0].added, '2026-10-15');
});
