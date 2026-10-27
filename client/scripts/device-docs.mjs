// Generates the measured part of the device table in docs/devices.md from
// site/data/device-results.json. The generated table sits between the
// <!-- device-results:start --> and <!-- device-results:end --> markers; the
// "To measure" table below it is written by hand.
//
//   node scripts/device-docs.mjs [--check] [--data site/data/device-results.json] [--docs docs/devices.md]
//
// --check exits non-zero when the docs differ from what the data would generate.
import { readFileSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { resolve } from 'node:path';

export const START = '<!-- device-results:start -->';
export const END = '<!-- device-results:end -->';
export const REPO_URL = 'https://github.com/RamazanKara/nightjar';
const CODECS = ['H.264', 'HEVC', 'AV1'];
const HEADER = [
  '| Device | SoC | Android | H.264 | HEVC | AV1 | Vendor keys declared | Notes |',
  '| --- | --- | --- | --- | --- | --- | --- | --- |'
];

// Only chips whose marketing name is certain. Anything else shows its raw model number.
const SOC_NAMES = {
  SM8750: 'Snapdragon 8 Elite',
  SM8650: 'Snapdragon 8 Gen 3',
  SM8550: 'Snapdragon 8 Gen 2',
  SM8475: 'Snapdragon 8+ Gen 1',
  SM8450: 'Snapdragon 8 Gen 1',
  SM8350: 'Snapdragon 888',
  SM8250: 'Snapdragon 865 or 870',
  SM7675: 'Snapdragon 7+ Gen 3'
};
const SOC_MAKERS = { QTI: 'Qualcomm', QUALCOMM: 'Qualcomm', MEDIATEK: 'MediaTek', SAMSUNG: 'Samsung' };

// Readable names for known vendor low-latency keys (without the "vendor." prefix).
const KEY_NAMES = {
  'qti-ext-dec-picture-order.enable': 'picture order',
  'qti-ext-output-fence.enable': 'output fence',
  'qti-ext-output-fence.fence_type': 'output fence',
  'qti-ext-dec-low-latency.enable': 'low latency',
  'mtk.vdec.cpu.boost.mode.value': 'CPU boost'
};

/** Escapes text for a markdown table cell. */
export function cell(text) {
  return String(text).replace(/[\\|[\]*_~]/g, '\\$&');
}

export function deviceName(entry) {
  const device = entry.results?.device ?? {};
  return entry.name || [device.manufacturer, device.model].filter(Boolean).join(' ') || 'Unknown device';
}

export function socName(device) {
  const model = device.socModel;
  if (!model) return device.hardware || '—';
  const known = SOC_NAMES[String(model).toUpperCase()];
  const maker = SOC_MAKERS[String(device.socManufacturer ?? '').toUpperCase()] ?? device.socManufacturer;
  if (known) return known + ' (' + model + ')';
  return [maker, model].filter(Boolean).join(' ');
}

/** "vendor.qti-ext-output-fence.enable" becomes "output fence"; unknown keys lose "vendor." and ".enable". */
export function keyLabel(key) {
  const bare = key.replace(/^vendor\./, '');
  return KEY_NAMES[bare] ?? '`' + bare.replace(/\.enable$/, '') + '`';
}

/** Summarizes the vendor keys per codec: shared keys plain, keys of some codecs only get an "(H.264 only)" suffix. */
export function vendorKeys(decoders) {
  const known = decoders.filter(decoder => Array.isArray(decoder.vendorLowLatencyKeys));
  if (!known.length) return '—';
  const perLabel = new Map();
  for (const decoder of known) {
    for (const label of new Set(decoder.vendorLowLatencyKeys.map(keyLabel))) {
      if (!perLabel.has(label)) perLabel.set(label, []);
      perLabel.get(label).push(decoder.codec);
    }
  }
  if (!perLabel.size) return 'None';
  return [...perLabel].map(([label, codecs]) => {
    if (codecs.length === known.length) return label;
    return label + ' (' + codecs.join(', ') + (codecs.length === 1 ? ' only)' : ')');
  }).join(', ');
}

function notes(entry) {
  const results = entry.results;
  const hz = results.latencyTest?.panelHz;
  const app = results.app?.version ? 'Nightjar ' + cell(results.app.version) : 'Nightjar';
  const tail = typeof hz === 'number' ? app + ', ' + hz + ' Hz.' : app + '; no latency test.';
  // `note` is an optional maintainer comment on the entry; the add script never sets it.
  return tail + (entry.note ? ' ' + cell(entry.note) : '') + ' [#' + entry.issue + '](' + REPO_URL + '/issues/' + entry.issue + ')';
}

function medianFor(entry, codec) {
  const benchmark = entry.results?.decoderBenchmark;
  if (!benchmark?.measured) return null;
  const decoder = (benchmark.results ?? []).find(item => item.codec === codec);
  return typeof decoder?.medianDecodeMs === 'number' ? decoder.medianDecodeMs : null;
}

export function rowFor(entry) {
  const device = entry.results.device ?? {};
  const benchmark = entry.results.decoderBenchmark;
  const decoders = benchmark?.measured ? benchmark.results ?? [] : [];
  const times = CODECS.map(codec => {
    const ms = medianFor(entry, codec);
    return ms === null ? '—' : ms.toFixed(2);
  });
  return '| ' + [
    cell(deviceName(entry)),
    cell(socName(device)),
    cell(device.android || '—'),
    ...times,
    vendorKeys(decoders),
    notes(entry)
  ].join(' | ') + ' |';
}

/** Entries sorted by fastest HEVC median (no HEVC time last), then by issue number. */
export function sortedEntries(table) {
  return [...table.entries].sort((a, b) => {
    const x = medianFor(a, 'HEVC') ?? Infinity;
    const y = medianFor(b, 'HEVC') ?? Infinity;
    return (x === y ? 0 : x < y ? -1 : 1) || a.issue - b.issue;
  });
}

/** The text between the markers, markers included, without a trailing newline. */
export function renderBlock(table) {
  return [
    START,
    '<!-- Generated from site/data/device-results.json by scripts/device-docs.mjs. Edit the data, not this table. -->',
    ...HEADER,
    ...sortedEntries(table).map(rowFor),
    END
  ].join('\n');
}

/** Returns the docs text with the generated block replaced; throws when the markers are missing. */
export function applyToDocs(docs, table) {
  const start = docs.indexOf(START);
  const end = docs.indexOf(END);
  if (start < 0 || end < start) throw new Error('docs are missing the ' + START + ' and ' + END + ' markers');
  return docs.slice(0, start) + renderBlock(table) + docs.slice(end + END.length);
}

function options(args) {
  const parsed = { data: 'site/data/device-results.json', docs: 'docs/devices.md', check: false };
  for (let i = 0; i < args.length; i++) {
    const arg = args[i];
    if (arg === '--check') parsed.check = true;
    else if (arg === '--data') parsed.data = args[++i];
    else if (arg === '--docs') parsed.docs = args[++i];
    else throw new Error('usage: device-docs.mjs [--check] [--data file] [--docs file]');
  }
  return parsed;
}

export function main(args, root = process.cwd()) {
  const opts = options(args);
  const table = JSON.parse(readFileSync(resolve(root, opts.data), 'utf8'));
  const file = resolve(root, opts.docs);
  const current = readFileSync(file, 'utf8');
  const next = applyToDocs(current, table);
  if (opts.check) {
    if (next !== current) throw new Error(opts.docs + ' is out of date; run: node scripts/device-docs.mjs');
    console.log(opts.docs + ' matches ' + opts.data + '.');
  } else if (next !== current) {
    writeFileSync(file, next);
    console.log('Updated ' + opts.docs + ' (' + table.entries.length + ' measured devices).');
  } else {
    console.log(opts.docs + ' is already up to date.');
  }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    main(process.argv.slice(2));
  } catch (error) {
    console.error('Error: ' + error.message);
    process.exitCode = 1;
  }
}
