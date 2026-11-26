import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { after, before, test } from 'node:test';
import { createTestServer } from './test-server.mjs';

// Rubylight 7.0's live command stage: what Home and Sessions show while a device streams.
globalThis.location = new URL('http://localhost/');
globalThis.window = new EventTarget();
let server, render, telemetry, Stage, Timeline, Sparklines;
before(async () => {
  server = await createTestServer();
  ({ render } = await server.ssrLoadModule('svelte/server'));
  telemetry = await server.ssrLoadModule('/src/components/overview/studio-telemetry.ts');
  ({ default: Stage } = await server.ssrLoadModule('/src/components/overview/LiveCommandStage.svelte'));
  ({ default: Timeline } = await server.ssrLoadModule('/src/components/overview/PipelineTimeline.svelte'));
  ({ default: Sparklines } = await server.ssrLoadModule('/src/components/overview/TelemetrySparklines.svelte'));
});
after(() => server?.close());

const source = name => readFileSync(new URL(`./src/${name}`, import.meta.url), 'utf8');
const timing = mean_ms => ({ mean_ms, p95_ms: mean_ms * 1.5, p99_ms: mean_ms * 2 });
const stages = (changes = {}) => ({
  frames: 600, window_seconds: 5, capture: timing(0.4), hold: timing(0.1), claim: timing(0.1), convert: timing(0.3),
  encode: timing(2.2), queue: timing(0.2), packetize: timing(0.1), send_queue: timing(0.2), send: timing(0.3),
  present_to_sent: timing(3.9), moonlight_host: timing(3.2), ...changes,
});
const stream = (changes = {}) => ({
  uuid: 'pixel', device_name: 'Pixel 9', role: 'stream', state: 'RUNNING', encoder: 'amf', warnings: [],
  width: 2400, height: 1080, fps: 120, video_format: 1, hdr: true, vrr: false, encoder_bitrate_kbps: 50_000,
  pyrowave_minimum_kbps: null, pyrowave_recommended_kbps: null, audio_channels: 2, frames_sent: 1, bytes_sent: 1,
  idr_requests: 0, uptime_seconds: 720,
  performance: {
    fps: 119.6, bitrate_mbps: 47.62, encode_mean_ms: 2.1, encode_p95_ms: 2.9, encode_p99_ms: 3.4,
    host_processing_mean_ms: 3.42, host_processing_p95_ms: 4.6, host_processing_p99_ms: 5.8, host_processing_max_ms: 9,
    frame_age_mean_ms: 4.1, frame_age_p95_ms: 6, present_to_send_mean_ms: 3.9, send_interval_p99_ms: 9, sample_frames: 240,
    stages: stages(),
    history: [{ fps: 118, bitrate_mbps: 40, host_processing_mean_ms: 3.1 }, { fps: 120, bitrate_mbps: 48, host_processing_mean_ms: 3.4 }],
  },
  ...changes,
});

test('the pills name the picture, the range, the codec and the encoder the host reports', () => {
  assert.deepEqual(telemetry.specPills(stream()), ['2400×1080', '120 fps', 'HDR', 'HEVC', 'AMD AMF']);
  assert.deepEqual(telemetry.specPills(stream({ hdr: false, vrr: true, video_format: 2, encoder: 'nvenc' })),
    ['2400×1080', '120 fps', 'VRR', 'AV1', 'NVIDIA NVENC']);
  // An encoder the console does not know is left out, never guessed.
  assert.deepEqual(telemetry.specPills(stream({ hdr: false, encoder: 'future' })), ['2400×1080', '120 fps', 'HEVC']);
});

test('the pipeline groups the host’s stages into four phases, proportional, and never invents one', () => {
  const phases = telemetry.pipelinePhases(stages(), true);
  assert.deepEqual(phases.map(phase => [phase.id, phase.label, Number(phase.ms.toFixed(2))]), [
    ['capture', 'Capture', 0.6], ['convert', 'Convert', 0.3], ['encode', 'Encode', 2.4], ['send', 'Pace and send', 0.6],
  ]);
  assert.ok(Math.abs(phases.reduce((sum, phase) => sum + phase.share, 0) - 1) < 1e-9);
  assert.ok(phases[2].share > phases[0].share);
  // A phase the host timed at nothing is left out, and the others share the bar.
  const without = telemetry.pipelinePhases(stages({ convert: timing(0) }), true);
  assert.deepEqual(without.map(phase => phase.id), ['capture', 'encode', 'send']);
  // No stage timings, a still picture, or no measured frame: no bar at all.
  assert.deepEqual(telemetry.pipelinePhases(undefined, true), []);
  assert.deepEqual(telemetry.pipelinePhases(stages({ frames: 0 }), true), []);
  assert.deepEqual(telemetry.pipelinePhases(stages(), false), []);
  assert.equal(render(Timeline, { props: { phases: [] } }).body.includes('Pipeline'), false);
  const drawn = render(Timeline, { props: { phases } }).body;
  assert.match(drawn, /aria-label="Time per new picture on this PC: Capture 0\.60 milliseconds, Convert 0\.30 milliseconds, Encode 2\.4 milliseconds, Pace and send 0\.60 milliseconds"/);
  assert.equal((drawn.match(/class="segment /g) ?? []).length, 4);
});

test('the traces are frame rate, bitrate and host frame time, a dash before a frame was measured', () => {
  const [fps, bitrate, host] = telemetry.traces(stream());
  assert.deepEqual([fps.label, fps.now, fps.unit, fps.sub, fps.values], ['Frame rate', '120', 'fps', 'Target 120', [118, 120]]);
  assert.deepEqual([bitrate.label, bitrate.now, bitrate.unit, bitrate.values], ['Bitrate', '47.6', 'Mbps', [40, 48]]);
  assert.deepEqual([host.label, host.now, host.sub, host.values], ['Host frame time', '3.4', '99% under 5.8 ms', [3.1, 3.4]]);
  const still = telemetry.traces(stream({ performance: { ...stream().performance, sample_frames: 0 } }));
  assert.deepEqual(still.map(trace => trace.now), ['–', '–', '–']);
  assert.equal(still[2].sub, 'No new pictures to time');
  // The host reports no round trip or jitter for a stream, so the stage shows none.
  assert.equal(telemetry.traces(stream()).some(trace => /jitter|rtt|round trip/i.test(trace.label)), false);
  const drawn = render(Sparklines, { props: { traces: telemetry.traces(stream()) } }).body;
  assert.equal((drawn.match(/<svg/g) ?? []).length, 3);
  assert.match(drawn, /aria-label="Bitrate over the last/);
});

test('stream health is one word and one sentence, from the host’s own findings', () => {
  const state = changes => telemetry.streamHealth(stream(changes));
  assert.deepEqual(state(), { level: 'healthy', label: 'Healthy', cause: 'Every stage on this PC is keeping up.' });
  assert.equal(state({ state: 'STOPPING' }).label, 'Attention');
  assert.deepEqual(state({ warnings: [{ code: 'x', message: 'The display changed its mode.' }] }),
    { level: 'attention', label: 'Attention', cause: 'The display changed its mode.' });
  const limited = { state: 'limited', reason: 'loss', ceiling_kbps: 50_000 };
  assert.equal(state({ encoder_bitrate_kbps: 22_000, link_rate: limited }).cause, 'The network lost packets, so the bitrate is down to 22 Mbps.');
  assert.match(state({ encoder_bitrate_kbps: 9_500, link_rate: { ...limited, reason: 'send_losses' } }).cause, /^This PC could not send fast enough, so the bitrate is down to 9\.5 Mbps\.$/);
  // Too little bitrate for PyroWave is the one thing the user has to act on.
  const starved = state({ pyrowave_minimum_kbps: 80_000, pyrowave_recommended_kbps: 120_000 });
  assert.deepEqual([starved.level, starved.label], ['action', 'Action needed']);
  assert.match(starved.cause, /Raise it to 120 Mbps or more on the device\.$/);
  assert.equal(state({ pyrowave_minimum_kbps: 30_000, pyrowave_recommended_kbps: 120_000 }).level, 'attention');
  const late = { windows: 20, pictures: 1000, late_pictures: 90, lost_ms: 120, dominant: 'encode', stages: [], not_measured: [] };
  assert.equal(state({ stutter: late }).cause, 'Some pictures leave this PC late, most of the time lost encoding it.');
  assert.equal(state({ stutter: { ...late, late_pictures: 10 } }).level, 'healthy');
  assert.equal(state({ performance: { ...stream().performance, sample_frames: 0 } }).cause, 'The picture is still: nothing new to send.');
  // The stage's one banner is the worst of its streams.
  assert.equal(telemetry.worstHealth([stream(), stream({ state: 'STOPPING' })]).level, 'attention');
  assert.equal(telemetry.worstHealth([]), null);
});

test('the stage leads with the first running stream and names the game on its device', () => {
  const tv = stream({ uuid: 'tv', device_name: 'TV', state: 'STOPPING' });
  assert.equal(telemetry.leadStream([tv, stream()]).uuid, 'pixel');
  assert.equal(telemetry.leadStream([tv]).uuid, 'tv');
  assert.equal(telemetry.stageTitle([stream()], 'Portal 2'), 'Portal 2 on Pixel 9');
  assert.equal(telemetry.stageTitle([tv, stream(), stream({ uuid: 'deck' })], null), 'Streaming to Pixel 9 and 2 more');
  assert.equal(telemetry.stageTitle([stream({ device_name: '' })], null), 'Streaming to a device');
});

test('copied diagnostics carry the figures and no address or device id', () => {
  const text = telemetry.diagnosticsText(stream(), 'Portal 2');
  assert.match(text, /^Rubylight stream: Portal 2\nPicture: 2400×1080, 120 fps, HDR, HEVC, AMD AMF\n/);
  assert.match(text, /Frame rate: 120 of 120 fps/);
  assert.match(text, /Pipeline: Capture 0\.60 ms, Convert 0\.30 ms, Encode 2\.4 ms, Pace and send 0\.60 ms/);
  assert.match(text, /Health: Healthy\. Every stage on this PC is keeping up\.$/);
  assert.doesNotMatch(text, /pixel|\d+\.\d+\.\d+\.\d+/);
});

test('the stage draws the health banner, the title, the deck and a row per stream', () => {
  const props = { streams: [stream(), stream({ uuid: 'tv', device_name: 'Living room TV' })], game: { name: 'Portal 2', app: null },
    status: { activeSessions: 2, appRunning: true, appName: 'Portal 2', paused: false, running: true }, ondisconnect() {} };
  const html = render(Stage, { props }).body;
  assert.match(html, /<section class="stage[^"]*" aria-label="Live stream">/);
  assert.match(html, /role="status"[^>]*>.*?<strong[^>]*>Healthy<\/strong>/s);
  assert.match(html, /<h2[^>]*>Portal 2 on Pixel 9 and 1 more<\/h2>/);
  for (const action of ['Stream details', 'Copy diagnostics', 'Quit game']) assert.match(html, new RegExp(`>${action}<`), action);
  assert.equal((html.match(/<article class="stream/g) ?? []).length, 2, 'every stream has its row, with its own Disconnect');
  // Without a running app there is nothing to quit; a missed poll marks the figures as old.
  const quiet = render(Stage, { props: { ...props, status: null, stale: true, label: 'Live now' } }).body;
  assert.doesNotMatch(quiet, />Quit game</);
  assert.match(quiet, /aria-label="Live now"/);
  assert.match(quiet, /these figures are a few seconds old/);
});

test('the stage is a media plane: its own palette in both themes, and nothing new asked of the host', () => {
  const stage = source('components/overview/LiveCommandStage.svelte');
  assert.match(stage, /--panel: var\(--media-panel-raised\);/);
  assert.match(stage, /--ink: var\(--media-ink\);/);
  assert.match(stage, /background: var\(--media-wash\);/);
  // Quit and disconnect use the calls the console already makes; the figures come from the page's poll.
  assert.deepEqual([...stage.matchAll(/api\.[a-zA-Z.]+\(/g)].map(match => match[0]), ['api.apps.coverUrl(', 'api.apps.close(']);
  assert.doesNotMatch(stage, /poll\(|setInterval|fetch\(/);
  for (const page of ['pages/Overview.svelte', 'pages/Sessions.svelte']) assert.match(source(page), /<LiveCommandStage label="/, page);
  // 2 columns of traces on a phone.
  assert.match(source('components/overview/TelemetrySparklines.svelte'), /@media \(max-width: 700px\) \{\s*\.traces \{\s*grid-template-columns: repeat\(2, minmax\(0, 1fr\)\);/);
});
