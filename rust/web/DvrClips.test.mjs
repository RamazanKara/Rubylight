// Stream DVR in the console (design 05, Console 2.0): the Clips page's words, its live panel in each
// state, the saved clips with their quota, and the calls behind them (/api/dvr*).
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

globalThis.location = new URL('http://localhost/');
globalThis.window = new EventTarget();
const server = await createTestServer();
const { render } = await server.ssrLoadModule('svelte/server');
const load = async (path) => (await server.ssrLoadModule(path)).default;
const view = await server.ssrLoadModule('/src/components/dvr/dvr-view.ts');
const apiModule = await server.ssrLoadModule('/src/lib/api.ts');
const { dvrEndpoints, clipVideoUrl, clipDownloadUrl, DVR_SAVE_TIMEOUT_MS } = await server.ssrLoadModule('/src/lib/api/dvr.ts');
const LivePanel = await load('/src/components/dvr/DvrLivePanel.svelte');
const ClipsPanel = await load('/src/components/dvr/ClipsPanel.svelte');
const Clips = await load('/src/pages/Clips.svelte');

const source = (path) => readFileSync(new URL(`./src/${path}`, import.meta.url), 'utf8');
const english = JSON.parse(readFileSync(new URL('../assets/locale/en.json', import.meta.url), 'utf8')).dvr;
const GB = 1024 ** 3;
const noop = () => {};
const retry = async () => {};

const status = (changes = {}) => ({
  state: 'armed', reason: 'none', ring_buffered_secs: 42, ring_capacity_secs: 60, recording: false,
  recording_elapsed_secs: 0, quota_used_bytes: 4.2 * GB, quota_limit_bytes: 20 * GB, clips_dir_available: true,
  container: 'mp4', enabled: true, clips_dir: 'C:\\ProgramData\\Butterpollo\\config\\clips', ...changes,
});
const clip = (changes = {}) => ({
  id: 'clip_0001abcd', app_title: 'Portal 2', created_unix_ms: 1_790_000_000_000, duration_ms: 60_000, width: 2560,
  height: 1440, fps: 120, codec: 'hevc', hdr: false, container: 'mp4', size_bytes: 84 * 1024 ** 2, kind: 'replay',
  recovered: false, ...changes,
});
const live = (dvr, props = {}) =>
  render(LivePanel, { props: { dvr, onsave: noop, onrecord: noop, onretry: retry, ...props } }).body;
const clipsPanel = (props) =>
  render(ClipsPanel, { props: { onpreview: noop, ondelete: noop, onmore: noop, onretry: retry, ...props } }).body;
/** The button whose label is `label`, with its attributes. */
const button = (html, label) => html.match(new RegExp(`<button[^>]*>(?:(?!</button>)[\\s\\S])*${label}`))?.[0] ?? '';

test('each refusal reads as the plain sentence the design gives, the same as the host locale', () => {
  assert.equal(view.reasonText('disabled'), 'Instant Replay is turned off on this PC. Enable Stream DVR in Rubylight settings to save clips.');
  assert.equal(view.reasonText('pyrowave'), 'Clips cannot be saved while streaming with PyroWave. Switch to H.264, HEVC, or AV1 to use Stream DVR.');
  assert.equal(view.reasonText('disk_full'), 'Not enough free disk space on the host PC to save this clip. Free up space or delete older clips.');
  assert.equal(view.reasonText('quota_exceeded'), view.reasonText('disk_full'));
  assert.equal(view.reasonText('no_keyframe_yet'), 'Waiting for the stream to capture a full frame. Try saving again in a moment.');
  assert.equal(view.reasonText('audio_codec_unsupported'), 'Clips require Opus audio. Switch audio codec from passthrough to Opus to record clips.');
  for (const reason of ['disabled', 'pyrowave', 'disk_full', 'no_keyframe_yet', 'audio_codec_unsupported', 'busy', 'io_error', 'not_streaming', 'unknown']) {
    assert.equal(view.reasonText(reason), english[`reason_${reason}`], `${reason} matches rust/assets/locale/en.json`);
  }
  // A reason a newer host adds still says something; no reason says nothing.
  assert.equal(view.reasonText('thermal_limit'), 'Clips are not available right now.');
  assert.equal(view.reasonText('none'), '');
  assert.equal(view.reasonText(undefined), '');
});

test('a refused action names the reason, or the host’s own words', () => {
  const { ApiError } = apiModule;
  assert.equal(view.refusalText(new ApiError(409, 'PyroWave stream', 'pyrowave')), view.reasonText('pyrowave'));
  assert.equal(view.refusalText(new ApiError(409, 'The clips folder is read-only', 'something_new')), 'The clips folder is read-only');
  assert.equal(view.refusalText(new ApiError(500, 'Internal error')), 'Internal error');
  assert.equal(view.refusalText('offline'), 'offline');
});

test('the live state is a word and a sentence, and the buttons follow it', () => {
  assert.deepEqual(view.liveStatus(status()), { status: 'ready', label: 'Replay ready' });
  assert.equal(view.liveLine(status()), 'Last 42 s of 60 s ready to save.');
  assert.ok(view.canSave(status()) && view.canRecord(status()));

  const recording = status({ state: 'recording', recording: true, recording_elapsed_secs: 134 });
  assert.deepEqual(view.liveStatus(recording), { status: 'streaming', label: 'Recording' });
  assert.equal(view.liveLine(recording), 'Recording 02:14. Last 42 s of 60 s ready to save.');
  assert.ok(view.canSave(recording) && view.canRecord(recording));

  const off = status({ state: 'off', reason: 'disabled', enabled: false, ring_buffered_secs: 0 });
  assert.deepEqual(view.liveStatus(off), { status: 'offline', label: 'Off' });
  assert.equal(view.liveLine(off), view.reasonText('disabled'));
  assert.ok(!view.canSave(off) && !view.canRecord(off));

  const idle = status({ state: 'off', reason: 'not_streaming', ring_buffered_secs: 0 });
  assert.deepEqual(view.liveStatus(idle), { status: 'idle', label: 'Not streaming' });
  assert.ok(!view.canSave(idle));

  const pyro = status({ state: 'unsupported', reason: 'pyrowave' });
  assert.deepEqual(view.liveStatus(pyro), { status: 'warning', label: 'Unavailable' });
  assert.equal(view.liveLine(pyro), view.reasonText('pyrowave'));
  assert.ok(!view.canSave(pyro) && !view.canRecord(pyro));

  const full = status({ state: 'error', reason: 'disk_full' });
  assert.deepEqual(view.liveStatus(full), { status: 'error', label: 'Cannot save' });
  assert.ok(!view.canSave(full));
  // A recording that hits the full disk can still be stopped.
  assert.ok(view.canRecord(status({ state: 'error', reason: 'disk_full', recording: true })));
  // A state a newer host adds is unavailable, not ready.
  assert.equal(view.liveStatus(status({ state: 'unknown', reason: 'unknown' })).label, 'Unavailable');
  assert.equal(view.bufferShare(status()), 0.7);
  assert.equal(view.bufferShare(status({ ring_capacity_secs: 0 })), 0);
});

test('clips and the quota read in the units people know', () => {
  assert.equal(view.clock(134), '02:14');
  assert.equal(view.clock(3725), '1:02:05');
  assert.equal(view.clipLength(45_000), '45 s');
  assert.equal(view.clipLength(90_000), '1 min 30 s');
  assert.equal(view.clipLength(120_000), '2 min');
  assert.equal(view.clipLength(3_720_000), '1 h 2 min');
  assert.equal(view.size(84 * 1024 ** 2), '84 MB');
  assert.equal(view.quotaText(4.2 * GB, 20 * GB), '4.2 GB of 20 GB used');
  assert.equal(view.quotaText(0, 0), '0 KB used');
  assert.ok(view.quotaNearFull(18 * GB, 20 * GB));
  assert.ok(!view.quotaNearFull(17.9 * GB, 20 * GB));
  assert.ok(!view.quotaNearFull(1, 0));
  assert.equal(view.codecLabel('h264'), 'H.264');
  assert.equal(view.codecLabel('hevc'), 'HEVC');
  assert.equal(view.codecLabel('av1'), 'AV1');
  assert.equal(view.pictureText(clip()), '2560×1440 · 120 fps');
  assert.equal(view.pictureText(clip({ fps: 59.94 })), '2560×1440 · 59.9 fps');
  assert.equal(view.pictureText(clip({ width: 0, height: 0, fps: 0 })), '–');
  assert.equal(view.kindLabel('replay'), 'Replay');
  assert.equal(view.kindLabel('manual'), 'Recording');
  assert.equal(view.kindLabel('later'), 'Clip');
  assert.equal(view.savedNotice(clip()), 'Clip saved (1 min, 84 MB).');
  assert.equal(view.savedNotice(clip({ kind: 'manual', duration_ms: 45_000 })), 'Recording saved (45 s, 84 MB).');
});

test('the calls use the routes, methods and bodies the host serves', async () => {
  const calls = [];
  const dvr = dvrEndpoints(async (method, path, body, timeout) => {
    calls.push({ method, path, body, timeout });
    return {};
  });
  await dvr.status();
  await dvr.clips();
  await dvr.clips('cur/sor+1');
  await dvr.saveClip();
  await dvr.saveClip(30);
  await dvr.record('start');
  await dvr.record('stop');
  await dvr.remove('clip_0001/../x');
  await dvr.openFolder();
  assert.deepEqual(calls, [
    { method: 'GET', path: '/api/dvr', body: undefined, timeout: undefined },
    { method: 'GET', path: '/api/dvr/clips?limit=50', body: undefined, timeout: undefined },
    { method: 'GET', path: '/api/dvr/clips?limit=50&cursor=cur%2Fsor%2B1', body: undefined, timeout: undefined },
    { method: 'POST', path: '/api/dvr/clip', body: {}, timeout: DVR_SAVE_TIMEOUT_MS },
    { method: 'POST', path: '/api/dvr/clip', body: { duration_secs: 30 }, timeout: DVR_SAVE_TIMEOUT_MS },
    { method: 'POST', path: '/api/dvr/record', body: { action: 'start' }, timeout: DVR_SAVE_TIMEOUT_MS },
    { method: 'POST', path: '/api/dvr/record', body: { action: 'stop' }, timeout: DVR_SAVE_TIMEOUT_MS },
    { method: 'DELETE', path: '/api/dvr/clips/clip_0001%2F..%2Fx', body: undefined, timeout: undefined },
    { method: 'POST', path: '/api/dvr/open-folder', body: {}, timeout: undefined },
  ]);
  assert.equal(clipVideoUrl('a b'), '/api/dvr/clips/a%20b/video');
  assert.equal(clipDownloadUrl('clip_0001abcd'), '/api/dvr/clips/clip_0001abcd/download');
});

test('api.dvr sends through the console’s request helper and keeps a refusal’s reason', async () => {
  const { api, ApiError, setCsrfToken } = apiModule;
  setCsrfToken('fixture-token');
  const sent = [];
  const realFetch = globalThis.fetch;
  globalThis.fetch = async (path, init) => {
    sent.push({ path, method: init.method, body: init.body, csrf: init.headers['X-CSRF-Token'] });
    return new Response(JSON.stringify({ status: false, error: 'PyroWave streams cannot be clipped', reason: 'pyrowave' }), {
      status: 409,
      headers: { 'Content-Type': 'application/json' },
    });
  };
  try {
    await assert.rejects(api.dvr.saveClip(), (error) => {
      assert.ok(error instanceof ApiError);
      assert.equal(error.status, 409);
      assert.equal(error.reason, 'pyrowave');
      assert.equal(error.message, 'PyroWave streams cannot be clipped');
      return true;
    });
  } finally {
    globalThis.fetch = realFetch;
  }
  assert.deepEqual(sent, [{ path: '/api/dvr/clip', method: 'POST', body: '{}', csrf: 'fixture-token' }]);
});

test('streaming with the buffer armed: Save clip and Start recording are ready', () => {
  const out = live(status());
  assert.match(out, /Instant Replay/);
  assert.match(out, /Replay ready/);
  assert.match(out, /Last 42 s of 60 s ready to save\./);
  assert.match(out, /role="meter"[^>]*aria-label="Replay buffer"/);
  assert.doesNotMatch(button(out, 'Save clip'), /disabled/);
  assert.doesNotMatch(button(out, 'Start recording'), /disabled/);
  assert.doesNotMatch(out, /Stop recording/);
});

test('recording: the timer runs and Stop recording replaces Start', () => {
  const out = live(status({ state: 'recording', recording: true, recording_elapsed_secs: 134 }));
  assert.match(out, /Recording/);
  assert.match(out, /aria-label="Recording time"[^>]*>02:14</);
  assert.doesNotMatch(button(out, 'Stop recording'), /disabled/);
  assert.doesNotMatch(out, /Start recording/);
});

test('turned off: the reason, a link to the setting, and the buttons disabled', () => {
  const out = live(status({ state: 'off', reason: 'disabled', enabled: false, ring_buffered_secs: 0 }));
  assert.match(out, /Instant Replay is turned off on this PC\./);
  assert.match(out, /href="\/settings\/picture\?setting=dvr_enabled"/);
  assert.match(button(out, 'Save clip'), /disabled/);
  assert.match(button(out, 'Start recording'), /disabled/);
  assert.doesNotMatch(out, /role="meter"/);
});

test('a PyroWave stream: unsupported, with the codec to switch to', () => {
  const out = live(status({ state: 'unsupported', reason: 'pyrowave' }));
  assert.match(out, /Unavailable/);
  assert.match(out, /Switch to H\.264, HEVC, or AV1 to use Stream DVR\./);
  assert.match(button(out, 'Save clip'), /disabled/);
  assert.match(button(out, 'Start recording'), /disabled/);
  assert.match(button(out, 'Save clip'), /title="Clips cannot be saved while streaming with PyroWave/);
});

test('before the first answer the panel says it is loading, and a failure offers Retry', () => {
  assert.match(live(null), /Loading Stream DVR's state…/);
  assert.match(live(null, { error: 'HTTP 500' }), /Could not load Stream DVR's state: HTTP 500/);
});

test('no clips yet: the empty state says how to save one', () => {
  const out = clipsPanel({ clips: [], quota: { used: 0, limit: 20 * GB }, dvr: status() });
  assert.match(out, /No clips yet/);
  assert.match(out, /Press Save clip here or in Nightjar's Control Center/);
  assert.match(out, /0 KB of 20 GB used/);
  const off = clipsPanel({ clips: [], quota: { used: 0, limit: 20 * GB }, dvr: status({ enabled: false }) });
  assert.match(off, /Turn on Instant Replay/);
});

test('saved clips: newest first with game, kind, codec, HDR, size and the row actions', () => {
  const clips = [
    clip(),
    clip({ id: 'clip_0002abcd', app_title: 'Forza Horizon 5', codec: 'av1', hdr: true, width: 3840, height: 2160, fps: 60, kind: 'manual',
      duration_ms: 754_000, size_bytes: 2.4 * GB }),
    clip({ id: 'clip_0003abcd', app_title: 'Hades II', codec: 'h264', recovered: true, kind: 'manual' }),
  ];
  const out = clipsPanel({ clips, quota: { used: 4.2 * GB, limit: 20 * GB }, dvr: status(), more: true });
  assert.match(out, /3\+ clips/);
  assert.match(out, /4\.2 GB of 20 GB used/);
  assert.ok(out.indexOf('Portal 2') < out.indexOf('Forza Horizon 5') && out.indexOf('Forza Horizon 5') < out.indexOf('Hades II'));
  for (const text of ['HEVC', 'AV1', 'H.264', 'HDR', 'Replay', 'Recording', 'Recovered', '84 MB', '2.4 GB', '12 min 34 s', '3840×2160 · 60 fps']) {
    assert.ok(out.includes(text), `shows ${text}`);
  }
  assert.match(out, /href="\/api\/dvr\/clips\/clip_0002abcd\/download"[^>]*download/);
  assert.match(out, />Preview<span class="visually-hidden[^"]*"> Portal 2<\/span>/);
  assert.match(out, />Delete<span class="visually-hidden[^"]*"> Hades II<\/span>/);
  assert.match(out, /Recovered clips were kept from a recording this PC could not finish/);
  assert.match(out, /Show older clips/);
  assert.match(out, /C:\\ProgramData\\Butterpollo\\config\\clips/);
  assert.doesNotMatch(out, /almost fill/);
});

test('a nearly full quota warns before the oldest replays go, with the setting to raise it', () => {
  const out = clipsPanel({ clips: [clip()], quota: { used: 19 * GB, limit: 20 * GB }, dvr: status() });
  assert.match(out, /19 GB of 20 GB used/);
  assert.match(out, /class="bar warn[ "]/);
  assert.match(out, /Clips almost fill their space\. The oldest replays are deleted/);
  assert.match(out, /href="\/settings\/picture\?setting=dvr_quota_gb"/);
});

test('an unreachable clips folder says so', () => {
  const out = clipsPanel({ clips: [], quota: { used: 0, limit: 20 * GB }, dvr: status({ clips_dir_available: false }) });
  assert.match(out, /This PC cannot reach the clips folder/);
});

test('Clips is a page under Advanced, routed from App.svelte, polling the DVR every 2 s', () => {
  const nav = source('lib/navigation.ts');
  const advanced = nav.slice(nav.indexOf('export const advanced'));
  assert.match(advanced, /path: '\/clips',\s*label: 'Clips',\s*icon: 'film'/);
  const app = source('App.svelte');
  assert.match(app, /import Clips from '\.\/pages\/Clips\.svelte';/);
  assert.match(app, /route\.path === '\/clips'\) return Clips;/);
  const page = source('pages/Clips.svelte');
  assert.match(page, /poll\(loadStatus, 2000\)/);
  assert.match(page, /poll\(loadClips, 15_000\)/);
  assert.match(page, /const id = \+\+latest;[\s\S]*?if \(id !== latest\) return;/);
  assert.match(page, /danger: true/);
  assert.match(source('components/dvr/ClipPreview.svelte'), /src=\{clipVideoUrl\(clip\.id\)\}/);
  assert.match(source('lib/api.ts'), /^export \* from '\.\/api\/dvr';$/m);
});

test('the Clips page draws its title, live panel and list without a host', () => {
  const out = render(Clips, { props: {} }).body;
  assert.match(out, /<h1[^>]*>Clips<\/h1>/);
  assert.match(out, /Open clips folder/);
  assert.match(out, /Loading Stream DVR's state…/);
  assert.match(out, /Loading the saved clips…/);
});
