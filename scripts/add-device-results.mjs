// Validates a pasted "nightjar-device-results" text (format version 1) and appends it
// to the device table data file of the site. Unknown fields are dropped, so only the
// documented fields are published.
//
//   node scripts/add-device-results.mjs <results.txt | -> --issue <number> [--name "Device name"]
//       [--data site/data/device-results.json] [--docs docs/devices.md] [--replace]
//       [--date YYYY-MM-DD] [--dry-run]
// --docs also regenerates the measured table in that file; --replace updates an issue that is
// already in the table instead of failing. The Device results workflow
// (.github/workflows/device-results.yml) does this automatically; this is the manual fallback.
import { readFileSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { resolve } from 'node:path';
import { applyToDocs } from './device-docs.mjs';

export const FORMAT = 'nightjar-device-results';
export const VERSION = 1;
export const TABLE_FORMAT = 'nightjar-device-table';
const MAX_TEXT = 96;
const MAX_DECODERS = 8;
const IDENTIFIER = /^[A-Za-z0-9._/-]{1,96}$/;
const VENDOR_KEY = /^vendor\.[A-Za-z0-9._-]{1,120}$/;
const CLIP = /^\d{2,5}x\d{2,5}@\d{1,3}$/;
const METRICS = ['frameIntervalMs', 'frameCallbackDelayMs', 'inputDispatchAgeMs', 'timerWakeupDelayMs'];
const DEVICE_TEXT = ['manufacturer', 'model', 'codename', 'socManufacturer', 'socModel', 'hardware', 'board', 'android'];

// Mirrors RedactedLog: anything the app would have replaced must not reach the table.
const SENSITIVE = [
  ['URL', /[a-zA-Z][a-zA-Z0-9+.-]*:\/\/\S+/],
  ['e-mail address', /[\w.+-]+@[\w-]+(?:\.[\w-]+)+/],
  ['IPv4 address', /\b\d{1,3}(?:\.\d{1,3}){3}\b/],
  ['IPv6 or MAC address', /[0-9a-fA-F]{1,4}(?::[0-9a-fA-F]{0,4}){2,7}/],
  ['UUID', /\b[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}\b/],
  ['long hex value', /\b[0-9a-fA-F]{16,}\b/],
  ['long token', /[A-Za-z0-9+/=_-]{32,}/],
  ['storage path', /(?:\/storage|\/sdcard|\/data\/user|\/data\/data|content:)/],
  ['secret', /\b(?:pin|password|passphrase|passcode|secret|token)\b/i],
  ['markup', /[<>`]/]
];

/** Problems that keep a free-text value out of the public table. */
export function textProblems(value) {
  const problems = [];
  if (value.length > MAX_TEXT) problems.push('longer than ' + MAX_TEXT + ' characters');
  if (/[\u0000-\u001f\u007f\u2028\u2029]/.test(value)) problems.push('contains control characters');
  for (const [name, pattern] of SENSITIVE) if (pattern.test(value)) problems.push('looks like a ' + name);
  return problems;
}

/** Extracts the JSON object from pasted text, for example an issue body with a code block. */
export function parseResults(text) {
  const start = text.indexOf('{');
  const end = text.lastIndexOf('}');
  if (start < 0 || end < start) throw new Error('no JSON object found');
  return JSON.parse(text.slice(start, end + 1));
}

function isObject(value) {
  return value !== null && typeof value === 'object' && !Array.isArray(value);
}

/**
 * Checks a parsed results object against format version 1 and returns
 * { errors, dropped, results } where results holds only the documented fields in format order.
 */
export function validateResults(input) {
  const errors = [];
  const dropped = [];
  const check = (path, ok, message) => {
    if (!ok) errors.push(path + ': ' + message);
    return ok;
  };
  const known = (path, object, keys) => {
    for (const key of Object.keys(object)) if (!keys.includes(key)) dropped.push(path + key);
  };
  const text = (path, value) => {
    if (value === null || value === undefined) return null;
    if (!check(path, typeof value === 'string', 'must be text or null')) return null;
    for (const problem of textProblems(value)) errors.push(path + ': ' + problem);
    return value;
  };
  const identifier = (path, value) => {
    if (value === null || value === undefined || value === '[omitted]') return value ?? null;
    check(path, typeof value === 'string' && IDENTIFIER.test(value), 'must be an identifier');
    return value;
  };
  const integer = (path, value, min, max, nullable) => {
    if (nullable && (value === null || value === undefined)) return null;
    check(path, Number.isInteger(value) && value >= min && value <= max, 'must be a whole number from ' + min + ' to ' + max);
    return value;
  };
  const decimal = (path, value, max) => {
    if (value === null || value === undefined) return null;
    check(path, typeof value === 'number' && Number.isFinite(value) && value >= 0 && value <= max, 'must be a number from 0 to ' + max + ' or null');
    return value;
  };
  const bool = (path, value, nullable) => {
    if (nullable && (value === null || value === undefined)) return null;
    check(path, typeof value === 'boolean', nullable ? 'must be true, false or null' : 'must be true or false');
    return value;
  };

  if (!check('', isObject(input), 'not a JSON object')) return { errors, dropped, results: null };
  check('format', input.format === FORMAT, 'must be "' + FORMAT + '"');
  check('version', input.version === VERSION, 'must be ' + VERSION + ' (this script reads version ' + VERSION + ' only)');
  known('', input, ['format', 'version', 'app', 'device', 'decoderBenchmark', 'latencyTest']);

  const results = { format: FORMAT, version: VERSION };
  const app = input.app;
  if (check('app', isObject(app), 'missing')) {
    known('app.', app, ['name', 'version', 'build']);
    check('app.name', app.name === 'Nightjar', 'must be "Nightjar"');
    results.app = {
      name: 'Nightjar',
      version: text('app.version', app.version),
      build: integer('app.build', app.build, 0, 1e9, false)
    };
  }

  const device = input.device;
  if (check('device', isObject(device), 'missing')) {
    known('device.', device, [...DEVICE_TEXT, 'sdk', 'gpu']);
    results.device = {};
    for (const key of DEVICE_TEXT) results.device[key] = text('device.' + key, device[key]);
    results.device.sdk = integer('device.sdk', device.sdk, 1, 100, false);
    results.device.gpu = text('device.gpu', device.gpu);
  }

  const benchmark = input.decoderBenchmark;
  if (check('decoderBenchmark', isObject(benchmark), 'missing')) {
    known('decoderBenchmark.', benchmark, ['measured', 'clip', 'results']);
    const out = { measured: bool('decoderBenchmark.measured', benchmark.measured, false) };
    if (benchmark.clip !== undefined) {
      check('decoderBenchmark.clip', typeof benchmark.clip === 'string' && CLIP.test(benchmark.clip), 'must look like 1920x1080@60');
      out.clip = benchmark.clip;
    }
    out.results = [];
    if (check('decoderBenchmark.results', Array.isArray(benchmark.results), 'must be a list')) {
      check('decoderBenchmark.results', benchmark.results.length <= MAX_DECODERS, 'more than ' + MAX_DECODERS + ' decoders');
      benchmark.results.slice(0, MAX_DECODERS).forEach((decoder, index) => {
        const path = 'decoderBenchmark.results[' + index + ']';
        if (!check(path, isObject(decoder), 'not an object')) return;
        known(path + '.', decoder, ['codec', 'mime', 'decoder', 'medianDecodeMs', 'featureLowLatency', 'vendorLowLatencyKeys']);
        const entry = {
          codec: text(path + '.codec', decoder.codec),
          mime: identifier(path + '.mime', decoder.mime),
          decoder: identifier(path + '.decoder', decoder.decoder),
          medianDecodeMs: decimal(path + '.medianDecodeMs', decoder.medianDecodeMs, 1000),
          featureLowLatency: bool(path + '.featureLowLatency', decoder.featureLowLatency, true),
          vendorLowLatencyKeys: null
        };
        const keys = decoder.vendorLowLatencyKeys;
        if (keys !== null && keys !== undefined) {
          if (check(path + '.vendorLowLatencyKeys', Array.isArray(keys) && keys.length <= 32, 'must be a list of at most 32 keys or null')) {
            keys.forEach(key => check(path + '.vendorLowLatencyKeys', typeof key === 'string' && VENDOR_KEY.test(key), 'not a vendor key: ' + JSON.stringify(key).slice(0, 40)));
            entry.vendorLowLatencyKeys = [...keys];
          }
        }
        out.results.push(entry);
      });
    }
    results.decoderBenchmark = out;
  }

  const latency = input.latencyTest;
  if (latency === null || latency === undefined) {
    results.latencyTest = null;
  } else if (check('latencyTest', isObject(latency), 'must be an object or null')) {
    known('latencyTest.', latency, ['panelHz', 'audioSampleRate', 'audioFramesPerBuffer', ...METRICS]);
    const out = {
      panelHz: decimal('latencyTest.panelHz', latency.panelHz, 1000),
      audioSampleRate: integer('latencyTest.audioSampleRate', latency.audioSampleRate, 1, 1e6, true),
      audioFramesPerBuffer: integer('latencyTest.audioFramesPerBuffer', latency.audioFramesPerBuffer, 1, 1e6, true)
    };
    for (const metric of METRICS) {
      const value = latency[metric];
      const path = 'latencyTest.' + metric;
      if (value === null || value === undefined) {
        out[metric] = null;
      } else if (check(path, isObject(value), 'must be an object or null')) {
        known(path + '.', value, ['samples', 'mean', 'p95', 'p99']);
        out[metric] = {
          samples: integer(path + '.samples', value.samples, 1, 1e7, false),
          mean: decimal(path + '.mean', value.mean, 1e5),
          p95: decimal(path + '.p95', value.p95, 1e5),
          p99: decimal(path + '.p99', value.p99, 1e5)
        };
      }
    }
    results.latencyTest = out;
  }
  return { errors, dropped, results: errors.length ? null : results };
}

export function emptyTable() {
  return { format: TABLE_FORMAT, version: 1, entries: [] };
}

/** Adds validated results to the table data; throws on duplicates or a bad name, issue or date. */
export function addEntry(table, results, args) {
  upsertEntry(table, results, args, false);
  return table;
}

/**
 * Like addEntry, but an issue that is already in the table has its name and results replaced
 * (the original `added` date stays). Returns 'added', 'updated' or 'unchanged'.
 */
export function replaceEntry(table, results, args) {
  return upsertEntry(table, results, args, true);
}

function upsertEntry(table, results, { issue, name, added }, replace) {
  if (!isObject(table) || table.format !== TABLE_FORMAT || table.version !== 1 || !Array.isArray(table.entries)) {
    throw new Error('the data file is not a ' + TABLE_FORMAT + ' version 1 file');
  }
  if (!Number.isInteger(issue) || issue <= 0) throw new Error('--issue must be the GitHub issue number');
  if (!/^\d{4}-\d{2}-\d{2}$/.test(added)) throw new Error('the date must be YYYY-MM-DD');
  let label = null;
  if (name !== undefined && name !== null) {
    label = String(name).trim();
    const problems = label ? textProblems(label) : ['empty'];
    if (problems.length) throw new Error('--name: ' + problems.join(', '));
  }
  const existing = table.entries.find(entry => entry.issue === issue);
  if (existing && !replace) throw new Error('issue #' + issue + ' is already in the table');
  const same = JSON.stringify(results);
  if (table.entries.some(entry => entry !== existing && JSON.stringify(entry.results) === same)) {
    throw new Error('these exact results are already in the table' + (existing ? ' under another issue' : ''));
  }
  if (!existing) {
    table.entries.push({ issue, added, name: label, results });
    return 'added';
  }
  if (JSON.stringify(existing.results) === same && (existing.name ?? null) === (label ?? existing.name ?? null)) return 'unchanged';
  if (label !== null) existing.name = label;
  existing.results = results;
  return 'updated';
}

function options(args) {
  const parsed = { data: 'site/data/device-results.json', added: new Date().toISOString().slice(0, 10), dryRun: false };
  for (let i = 0; i < args.length; i++) {
    const arg = args[i];
    if (arg === '--issue') parsed.issue = Number(args[++i]);
    else if (arg === '--name') parsed.name = args[++i];
    else if (arg === '--data') parsed.data = args[++i];
    else if (arg === '--date') parsed.added = args[++i];
    else if (arg === '--docs') parsed.docs = args[++i];
    else if (arg === '--replace') parsed.replace = true;
    else if (arg === '--dry-run') parsed.dryRun = true;
    else if (parsed.input === undefined) parsed.input = arg;
    else throw new Error('unexpected argument ' + arg);
  }
  if (parsed.input === undefined) throw new Error('usage: add-device-results.mjs <results.txt | -> --issue <number> [--name "Device name"] [--data file] [--docs docs/devices.md] [--replace] [--date YYYY-MM-DD] [--dry-run]');
  return parsed;
}

export function main(args, root = process.cwd()) {
  const opts = options(args);
  const pasted = readFileSync(opts.input === '-' ? 0 : resolve(root, opts.input), 'utf8');
  const { errors, dropped, results } = validateResults(parseResults(pasted));
  for (const field of dropped) console.warn('Dropped unknown field ' + field);
  if (errors.length) throw new Error('the results do not pass the checks:\n  ' + errors.join('\n  '));
  const dataFile = resolve(root, opts.data);
  const table = JSON.parse(readFileSync(dataFile, 'utf8'));
  const action = opts.replace ? replaceEntry(table, results, opts) : (addEntry(table, results, opts), 'added');
  const output = JSON.stringify(table, null, 2) + '\n';
  const docsFile = opts.docs ? resolve(root, opts.docs) : null;
  const docs = docsFile ? applyToDocs(readFileSync(docsFile, 'utf8'), table) : null;
  if (opts.dryRun) {
    process.stdout.write(JSON.stringify(table.entries.find(entry => entry.issue === opts.issue), null, 2) + '\n');
  } else {
    writeFileSync(dataFile, output);
    if (docsFile) writeFileSync(docsFile, docs);
    console.log(action[0].toUpperCase() + action.slice(1) + ' issue #' + opts.issue + ' in ' + opts.data + ' (' + table.entries.length + ' entries).');
  }
  return table;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    main(process.argv.slice(2));
  } catch (error) {
    console.error('Error: ' + error.message);
    process.exitCode = 1;
  }
}
