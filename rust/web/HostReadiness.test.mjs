import assert from 'node:assert/strict';
import { test } from 'node:test';
import { readFileSync } from 'node:fs';
import { createTestServer } from './test-server.mjs';

globalThis.location = new URL('http://localhost/');
globalThis.window = new EventTarget();
const server = await createTestServer();
const { default: HostReadiness } = await server.ssrLoadModule('/src/components/overview/HostReadiness.svelte');
const { render } = await server.ssrLoadModule('svelte/server');
const { needsAttention, problems, homeLead, wakeCheck, hostChecks } = await server.ssrLoadModule('/src/lib/host-readiness.ts');
const metadata = {
  encoder_status: { state: 'failed', h264: false, hevc: false, av1: false, pyrowave: false },
  warnings: [{ code: 'video_encoder', message: 'No video encoder available: AMF error 1. Retrying.' }],
  virtual_display: { capable: false, ready: false, reason: '' },
  capture_status: { virtual_display_configured: false, displays: [], error: null },
  audio_enabled: false,
};
const html = (changes = {}) => render(HostReadiness, {
  props: { metadata: { ...metadata, ...changes }, error: null, checking: false, onrecheck: () => {} },
}).body;

test('a host warning alone makes Home show the checks and the warning', () => {
  const healthy = {
    ...metadata,
    warnings: [],
    encoder_status: { ...metadata.encoder_status, state: 'ready', h264: true, hevc: true },
    virtual_display: { capable: true, ready: true, reason: '' },
    capture_status: { virtual_display_configured: true, displays: [{ name: 'Display 1' }], error: null },
    audio_enabled: true,
    audio_sinks: [{ name: 'Speakers', description: 'Speakers', default: true }],
  };
  assert.equal(needsAttention(healthy), false);
  const firewall = { code: 'firewall_blocked', message: 'Windows Firewall rule "x" blocks incoming connections to the host.' };
  assert.equal(needsAttention({ ...healthy, warnings: [firewall] }), true);
  // Home lists the problems the hero doesn't name, ranked as the hero ranks them.
  const overview = readFileSync(new URL('./src/pages/Overview.svelte', import.meta.url), 'utf8');
  assert.match(overview, /found = \$derived\(metadata \? problems\(metadata, wakePower, fixes\.dismissed\)/);
  assert.match(overview, /showChecks = \$derived\(detailsOpen \|\| found\.length > \(heroProblem \? 1 : 0\)\)/);
});

test('host readiness shows the probe error even while a retry is running', () => {
  for (const state of ['failed', 'checking']) {
    const result = html({ encoder_status: { ...metadata.encoder_status, state } });
    assert.match(result, /No video encoder available: AMF error 1\. Retrying\./);
    assert.match(result, /Check the selected encoder and graphics adapter/);
    assert.match(result, /href="\/settings\/picture"/);
    assert.doesNotMatch(result, /restart the host|Testing which codecs/);
  }
});

test('host readiness clears the warning when encoders recover', () => {
  const result = html({ warnings: [], encoder_status: { ...metadata.encoder_status, state: 'ready', h264: true } });
  assert.doesNotMatch(result, /No video encoder available|Retrying/);
  assert.match(result, /Encodes H\.264\./);
});

test('host readiness names the YUV 4:4:4 modes the encoder check found', () => {
  const ready = { ...metadata.encoder_status, state: 'ready', h264: true, hevc: true, av1: true };
  const full = html({ warnings: [], encoder_status: { ...ready, yuv444: ['H.264 4:4:4', 'HEVC 4:4:4', 'HEVC 10-bit 4:4:4'] } });
  assert.match(full, /Encodes H\.264, HEVC, and AV1\. YUV 4:4:4 in H\.264, HEVC, and HEVC 10-bit\./);
  for (const yuv444 of [[], undefined]) {
    const plain = html({ warnings: [], encoder_status: { ...ready, yuv444 } });
    assert.match(plain, /Encodes H\.264, HEVC, and AV1\./);
    assert.doesNotMatch(plain, /4:4:4/);
  }
});

test('host readiness escapes driver errors', () => {
  const result = html({ warnings: [{ code: 'video_encoder', message: 'No video encoder available: <script>alert(1)</script>. Retrying.' }] });
  assert.match(result, /&lt;script/);
  assert.doesNotMatch(result, /<script>alert/);
});

test('host readiness names driver push and explains its display scope without claiming it is active', () => {
  const capture_status = { ...metadata.capture_status, configured_backend: 'idd-push', displays: [{ name: 'DISPLAY1' }] };
  const result = html({ capture_status });
  assert.match(result, /Prefers Direct driver push on Rubylight virtual displays/);
  assert.match(result, /Windows capture as fallback/);
  assert.doesNotMatch(result, /Uses the configured capture method/);
});

test('host readiness writes notes in the text face and opens them only for problems', () => {
  const reason = 'Only the Rubylight service may open the virtual display driver.';
  const healthy = { warnings: [], encoder_status: { ...metadata.encoder_status, state: 'ready', h264: true } };
  const off = html({ ...healthy, virtual_display: { capable: false, ready: false, reason } });
  assert.match(off, new RegExp(`<p class="detail[^"]*">${reason}</p>`));
  assert.doesNotMatch(off, /class="[^"]*\bmono\b/);
  assert.doesNotMatch(off, /<details open/);
  const broken = html({
    ...healthy,
    virtual_display: { capable: false, ready: false, reason },
    capture_status: { ...metadata.capture_status, virtual_display_configured: true },
  });
  assert.match(broken, /<details open[^>]*>\s*<summary[^>]*>Details<\/summary>\s*<p class="detail[^"]*">Only the Rubylight/);
  assert.match(broken, /Check that the Rubylight service is running/);
});

test('host readiness lists the host warnings with a way to dismiss each', () => {
  const healthy = {
    encoder_status: { ...metadata.encoder_status, state: 'ready', h264: true },
    capture_status: { ...metadata.capture_status, displays: [{ name: 'DISPLAY1' }] },
  };
  const result = html({
    ...healthy,
    warnings: [
      { code: 'video_encoder', message: 'No video encoder available: AMF error 1. Retrying.' },
      { code: 'settings_retired', message: 'Replaced settings from Apollo: <encoder>.' },
      { code: 'firewall_blocked', message: 'Windows Firewall rule "butterpollo.exe" blocks incoming connections to the host.' },
    ],
  });
  assert.match(result, /Settings replaced/);
  assert.match(result, /Replaced settings from Apollo: &lt;encoder>\./);
  assert.match(result, /Windows Firewall blocks Rubylight/);
  assert.equal(result.match(/>Dismiss</g)?.length, 2, 'the encoder warning stays with its check');
  assert.match(result, /Needs attention/);
  const quiet = html({ ...healthy, warnings: [] });
  assert.doesNotMatch(quiet, /Dismiss|Needs attention/);
});

// A PC whose checks all pass, for the tests below to break one thing at a time.
const ready = {
  paired_devices: 1,
  warnings: [],
  encoder_status: { state: 'ready', h264: true, hevc: true, av1: false, pyrowave: false },
  virtual_display: { capable: true, ready: true, reason: '' },
  capture_status: { virtual_display_configured: true, displays: [{ name: 'DISPLAY1' }], error: null },
  audio_enabled: true,
  audio_sinks: [{ name: 'Speakers', description: 'Speakers', default: true }],
};
const wake = (state, changes = ['Realtek: allow this device to wake the computer'], running = false) => ({
  wake: { primary: 'aa', targets: [{ mac: 'aa', kind: 'ethernet', connected: true, route: true, broadcasts: [], state, checks: [] }],
    system: { sleep: 's3', modern_standby: false, hibernate: false, fast_startup: 'off', shutdown_wake: 'unknown' } },
  idle_sleep: { state: 'off', minutes: 0, reason: null, remaining_seconds: null },
  last_sleep: null,
  fix: { running, changes, skipped: [], error: null, fast_startup_off_offered: false, last: { at: 41, errors: [] } },
});

test('the checks list only problems, each with its fix, and fold what is fine into one line', () => {
  const result = render(HostReadiness, { props: {
    metadata: { ...ready, audio_sinks: [] }, power: wake('needs_setup'), error: null, checking: false, onrecheck: () => {},
  } }).body;
  assert.match(result, /Needs attention/);
  assert.doesNotMatch(result, /items? needs? attention\./, 'the title says it once');
  assert.match(result, />Turn on<span class="visually-hidden[^"]*">: Wake from a device</);
  assert.match(result, /No audio output was found on this PC\./);
  // Speakers to connect is no instruction a button could follow: Audio settings is the way.
  assert.doesNotMatch(result, /Connect speakers or headphones/);
  assert.match(result, /href="\/settings\/sound"/);
  assert.match(result, /<details[^>]*>\s*<summary[^>]*><span class="ok-dot[^"]*" aria-hidden="true"><\/span>3 checks ready<\/summary>/);
  const fine = render(HostReadiness, { props: { metadata: ready, power: wake('ready'), error: null, checking: false, onrecheck: () => {} } }).body;
  assert.match(fine, />Checks</);
  assert.match(fine, /Ready to stream\./);
  assert.doesNotMatch(fine, /Needs attention|checks ready|>Turn on</);
  assert.match(fine, /Devices can wake this PC from sleep\./);
});

test('what stops every stream comes first, then what one click fixes, then the rest, host warnings last', () => {
  const firewall = { code: 'firewall_blocked', message: 'Windows Firewall blocks the host.' };
  const found = problems({ ...ready, audio_sinks: [], warnings: [firewall], capture_status: { ...ready.capture_status, displays: [], virtual_display_configured: false } }, wake('needs_setup'));
  assert.deepEqual(found.map((check) => check.id), ['capture', 'wake', 'audio', 'warning:firewall_blocked']);
  // No monitor is on and the virtual display is ready but unused: one click streams it.
  assert.equal(found[0].blocking, true);
  assert.deepEqual(found[0].fix, { kind: 'settings', label: 'Use a virtual display', values: { virtual_display_mode: 'per_client' }, done: 'Streams use a virtual display.' });
  assert.deepEqual(found[1].fix, { kind: 'wake', label: 'Turn on', changes: ['Realtek: allow this device to wake the computer'], previous: 41 });
  assert.equal(found[3].headline, 'Windows Firewall blocks Rubylight');
  // A dismissed warning is gone at once, until its text changes.
  assert.deepEqual(problems({ ...ready, warnings: [firewall] }, null, [`${firewall.code}\n${firewall.message}`]), []);
  assert.equal(problems({ ...ready, warnings: [{ ...firewall, message: 'Another rule blocks the host.' }] }, null, [`${firewall.code}\n${firewall.message}`]).length, 1);
});

test('a broken virtual display offers the monitor that is on, and blocks streams without one', () => {
  const broken = { ...ready, virtual_display: { capable: false, ready: false, reason: '' } };
  const [withMonitor] = problems(broken);
  assert.equal(withMonitor.id, 'virtual-display');
  assert.equal(withMonitor.blocking, false);
  assert.deepEqual(withMonitor.fix?.values, { virtual_display_mode: 'disabled' });
  const without = problems({ ...broken, capture_status: { ...broken.capture_status, displays: [] } });
  assert.equal(without[0].id, 'virtual-display');
  assert.equal(without[0].blocking, true);
  assert.equal(without[0].fix, undefined);
  assert.match(without[0].help, /turn off the virtual display/);
});

test('wake shows only when one click can change it, and only once a device is paired', () => {
  assert.equal(wakeCheck(wake('needs_setup')).fix.kind, 'wake');
  assert.equal(wakeCheck(wake('ready')).tone, 'ok');
  for (const power of [wake('needs_setup', []), wake('needs_setup', ['x'], true), wake('experimental'), wake('unknown')]) {
    assert.equal(wakeCheck(power), null);
  }
  assert.equal(hostChecks(ready).some((check) => check.id === 'wake'), false);
  const overview = readFileSync(new URL('./src/pages/Overview.svelte', import.meta.url), 'utf8');
  assert.match(overview, /wakePower = \$derived\(paired \? power : null\)/);
  // Read on arrival and on Recheck: never polled.
  assert.doesNotMatch(overview, /poll\(loadPower/);
});

test('Home leads with a stream, then what stops streams, then pairing, then a problem that only limits them', () => {
  const lead = (changes) => homeLead({ unavailable: false, loaded: true, live: false, paired: true, probing: false, top: undefined, ...changes });
  const audio = problems({ ...ready, audio_sinks: [] })[0];
  const encoder = problems({ ...ready, encoder_status: { state: 'failed' } })[0];
  assert.equal(lead({}), 'ready');
  assert.equal(lead({ top: audio }), 'problem');
  // An unpaired PC pairs first: a missing speaker doesn't stop that.
  assert.equal(lead({ paired: false, top: audio }), 'pair');
  assert.equal(lead({ paired: false, top: encoder }), 'blocked');
  assert.equal(lead({ live: true, top: encoder }), 'live');
  assert.equal(lead({ loaded: false }), 'checking');
  assert.equal(lead({ probing: true }), 'checking');
  assert.equal(lead({ unavailable: true, live: true }), 'unavailable');
});
