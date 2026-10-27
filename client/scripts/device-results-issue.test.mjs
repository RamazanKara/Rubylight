import assert from 'node:assert/strict';
import test from 'node:test';
import { emptyTable } from './add-device-results.mjs';
import { MARKER, evaluate, failureComment, isDeviceResultsIssue, parseIssue, section, successComment } from '../.github/scripts/device-results-issue.mjs';

const results = {
  format: 'nightjar-device-results',
  version: 1,
  app: { name: 'Nightjar', version: '0.8.0', build: 340 },
  device: {
    manufacturer: 'vivo', model: 'I2012', codename: 'PD2057', socManufacturer: 'Qualcomm', socModel: 'SM8250',
    hardware: 'qcom', board: 'kona', android: '14', sdk: 34, gpu: 'Adreno (TM) 650'
  },
  decoderBenchmark: {
    measured: true,
    clip: '1920x1080@60',
    results: [{ codec: 'HEVC', mime: 'video/hevc', decoder: 'OMX.qcom.video.decoder.hevc', medianDecodeMs: 2.83, featureLowLatency: false, vendorLowLatencyKeys: [] }]
  },
  latencyTest: null
};

const TICKED = '- [x] I checked the results text and agree that it is published in this issue and in the public device table.';
const UNTICKED = '- [ ] I checked the results text and agree that it is published in this issue and in the public device table.';

function form({ device = 'iQOO 7', text = '```json\n' + JSON.stringify(results, null, 2) + '\n```', conditions = 'Plugged in', stream = '_No response_', consent = TICKED } = {}) {
  return `### Device\n\n${device}\n\n### Results text\n\n${text}\n\n### Test conditions\n\n${conditions}\n\n### Stream on this device (optional)\n\n${stream}\n\n### Publishing\n\n${consent}`;
}

const run = (body, table = emptyTable(), issue = 25) => ({ table, ...evaluate({ issue, body, table, today: '2026-10-24' }) });

test('sections come out of an issue form, also with Windows line endings', () => {
  const body = form({ device: 'Pixel 8', conditions: 'Cool\n\nwith more lines' });
  assert.equal(section(body, 'Device'), 'Pixel 8');
  assert.equal(section(body, 'Test conditions'), 'Cool\n\nwith more lines');
  assert.equal(section(body.replace(/\n/g, '\r\n'), 'Device'), 'Pixel 8');
  assert.equal(section(body, 'Missing'), '');
  assert.equal(section(body, 'Publishing'), TICKED);
});

test('parseIssue reads name, results, stream notes and consent', () => {
  const parsed = parseIssue(form({ device: '  Pixel 8  ' }));
  assert.equal(parsed.name, 'Pixel 8');
  assert.ok(parsed.resultsText.startsWith('```json'));
  assert.equal(parsed.stream, '');
  assert.equal(parsed.consent, true);
  assert.equal(parseIssue(form({ consent: UNTICKED })).consent, false);
  assert.equal(parseIssue(form({ stream: 'HEVC 4K at 60, decode 9 ms' })).stream, 'HEVC 4K at 60, decode 9 ms');
  assert.equal(parseIssue(null).name, '');
  assert.equal(parseIssue('### Device\n\n_No response_\n').name, '');
});

test('only open issues with the device-results label are handled', () => {
  const issue = { state: 'open', labels: [{ name: 'device-results' }] };
  assert.equal(isDeviceResultsIssue(issue), true);
  assert.equal(isDeviceResultsIssue({ ...issue, state: 'closed' }), false);
  assert.equal(isDeviceResultsIssue({ ...issue, labels: [{ name: 'bug' }] }), false);
  assert.equal(isDeviceResultsIssue({ ...issue, pull_request: {} }), false);
  assert.equal(isDeviceResultsIssue(undefined), false);
});

test('a good form is added with the name, the date and no hold', () => {
  const { status, action, hold, table, name } = run(form());
  assert.equal(status, 'ok');
  assert.equal(action, 'added');
  assert.equal(hold, false);
  assert.equal(name, 'iQOO 7');
  assert.deepEqual(table.entries.map(({ issue, added, name }) => ({ issue, added, name })), [{ issue: 25, added: '2026-10-24', name: 'iQOO 7' }]);
});

test('results pasted without the code fence work too', () => {
  assert.equal(run(form({ text: JSON.stringify(results, null, 2) })).status, 'ok');
});

test('stream notes keep the issue open', () => {
  const outcome = run(form({ stream: 'Frames drop after a minute' }));
  assert.equal(outcome.status, 'ok');
  assert.equal(outcome.hold, true);
  assert.match(successComment(outcome), /leaving this issue open/);
});

test('missing consent, name and results are reported together and nothing is added', () => {
  const outcome = run(form({ device: '_No response_', text: '_No response_', consent: UNTICKED }));
  assert.equal(outcome.status, 'invalid');
  assert.equal(outcome.problems.length, 3);
  assert.match(outcome.problems.join('\n'), /Publishing/);
  assert.match(outcome.problems.join('\n'), /"Device" field is empty/);
  assert.match(outcome.problems.join('\n'), /"Results text" field is empty/);
  assert.equal(outcome.table.entries.length, 0);
});

test('broken JSON, failed checks and unsafe names are explained', () => {
  const broken = run(form({ text: '{"format": "nightjar-device-results", ' }));
  assert.equal(broken.status, 'invalid');
  assert.match(broken.problems[0], /not valid JSON/);

  const noJson = run(form({ text: 'I could not copy it' }));
  assert.match(noJson.problems[0], /no JSON object found/);

  const leaked = structuredClone(results);
  leaked.device.board = 'mail me@example.com';
  const leak = run(form({ text: JSON.stringify(leaked) }));
  assert.equal(leak.status, 'invalid');
  assert.match(leak.problems[0], /device\.board: looks like a e-mail address/);

  const unsafe = run(form({ device: 'See http://example.com' }));
  assert.match(unsafe.problems.join('\n'), /Device name: looks like a URL/);
  assert.equal(unsafe.table.entries.length, 0);
});

test('an edit of an issue already in the table replaces its entry and keeps the date', () => {
  const table = emptyTable();
  assert.equal(run(form(), table).action, 'added');
  assert.equal(evaluate({ issue: 25, body: form(), table, today: '2026-11-30' }).action, 'unchanged');

  const faster = structuredClone(results);
  faster.decoderBenchmark.results[0].medianDecodeMs = 2.5;
  const edited = evaluate({ issue: 25, body: form({ device: 'iQOO 7 (Snapdragon 870)', text: JSON.stringify(faster) }), table, today: '2026-11-30' });
  assert.equal(edited.action, 'updated');
  assert.equal(table.entries.length, 1);
  assert.equal(table.entries[0].added, '2026-10-24');
  assert.equal(table.entries[0].name, 'iQOO 7 (Snapdragon 870)');
  assert.equal(table.entries[0].results.decoderBenchmark.results[0].medianDecodeMs, 2.5);
});

test('the same results from a different issue are refused', () => {
  const table = emptyTable();
  run(form(), table, 25);
  const other = run(form(), table, 30);
  assert.equal(other.status, 'invalid');
  assert.match(other.problems[0], /exact results are already in the table/);
  assert.equal(table.entries.length, 1);
});

test('comments carry the marker, the links and no user text beyond the problems', () => {
  const ok = successComment({ action: 'added', hold: false, dropped: [] });
  assert.ok(ok.startsWith(MARKER));
  assert.match(ok, /ramazankara\.github\.io\/nightjar\/devices\.html/);
  assert.match(ok, /docs\/devices\.md#devices/);
  assert.match(ok, /Open a new "Device results" issue/);
  assert.match(ok, /reply here and it will be reopened/);
  assert.match(successComment({ action: 'updated', hold: false, dropped: ['extra'] }), /updated your results[\s\S]*`extra`/);

  const bad = failureComment(Array.from({ length: 15 }, (_, i) => 'problem ' + i));
  assert.ok(bad.startsWith(MARKER));
  assert.match(bad, /- problem 11\n/);
  assert.doesNotMatch(bad, /problem 12/);
  assert.match(bad, /and 3 more/);
  assert.match(bad, /edit this issue/);
});

test('mentions and backticks in a problem are defused', () => {
  const leaked = structuredClone(results);
  leaked.decoderBenchmark.results[0].vendorLowLatencyKeys = ['@maintainer `x`'];
  const outcome = run(form({ text: JSON.stringify(leaked) }));
  assert.equal(outcome.status, 'invalid');
  assert.doesNotMatch(outcome.problems.join('\n'), /@maintainer/);
  assert.doesNotMatch(outcome.problems.join('\n'), /`/);
});
