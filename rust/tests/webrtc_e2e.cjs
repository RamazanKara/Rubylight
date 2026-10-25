// End-to-end legs for the WebRTC server (docs/webrtc-design.md, "Testing").
// Starts an isolated test host (C:\tests\webrtc, ports 48140 and 48545+),
// pairs, lists apps and streams to real browsers. Never touches the installed
// host except to read its /serverinfo: every run starts with that idle check.
//
//   node rust/tests/webrtc_e2e.cjs [--legs pair,apps,...] [--seconds 60]
//
// Requires Playwright (PLAYWRIGHT_MODULE), Edge or Chrome and Windows.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const {execFileSync, spawn} = require('node:child_process');
const {Signalling} = require('./webrtc_client/signalling.js');
const hostSupport = require('./webrtc_client/host.cjs');
const support = require('./webrtc_client/support.cjs');
const {chromium} = require(process.env.PLAYWRIGHT_MODULE || 'playwright');

const args = process.argv.slice(2);
const option = (name, fallback) => { const i = args.indexOf('--' + name); return i >= 0 ? args[i + 1] : fallback; };
const ALL_LEGS = ['pair', 'apps', 'browser', 'av1', 'hevc', 'surround51', 'surround71'];
const legs = option('legs', ALL_LEGS.join(',')).split(',');
const seconds = Number(option('seconds', 60));
// `--runs 3` repeats every streaming leg; `--system` starts the test host as SYSTEM in the console
// session (the virtual display driver accepts nothing else); `--host-config a=b,c=d` (a value may hold commas: 30,60) adds lines to
// the test host's sunshine.conf, so one invocation measures one host configuration.
const runs = Number(option('runs', 1));
const system = args.includes('--system');
const hostConfig = Object.fromEntries(option('host-config', '').split(/,(?=\w+=)/).filter(Boolean).map(pair => {
  const at = pair.indexOf('=');
  assert(at > 0, `--host-config wants key=value pairs, not "${pair}"`);
  return [pair.slice(0, at), pair.slice(at + 1)];
}));
const WEB_PORT = 48570, STATIC_PORT = 48571;
const client = path.join(__dirname, 'webrtc_client');
const script = name => fs.readFileSync(path.join(client, name), 'utf8');
const stamp = new Date().toISOString().replace(/[-:]/g, '').replace(/\..*/, '');
const output = option('out', process.env.BUTTERPOLLO_TEST_ARTIFACTS || path.join(hostSupport.defaults.root, 'runs', stamp));
const ctx = {results: {}, notes: []};

// What must be undone if the run ends any way but the normal one: Ctrl+C, a closed console, a
// crash. Each entry is synchronous, because it runs while the process exits. A run killed outright
// leaves it to the next start (`recoverFromEarlierRun`).
const cleanups = new Set();
function onExit(fn) { cleanups.add(fn); return () => cleanups.delete(fn); }
function runCleanups() { for (const fn of [...cleanups]) { cleanups.delete(fn); try { fn(); } catch {} } }
process.on('exit', runCleanups);
for (const signal of ['SIGINT', 'SIGTERM', 'SIGBREAK', 'SIGHUP']) process.on(signal, () => { runCleanups(); process.exit(130); });
const killTree = pid => { if (pid) { try { execFileSync('taskkill', ['/PID', String(pid), '/T', '/F'], {stdio: 'ignore'}); } catch {} } };

const signalling = host => `ws://127.0.0.1:${host.o.webrtcPort}`;
const sleep = ms => new Promise(r => setTimeout(r, ms));

function credentialsFile(host) { return path.join(host.o.root, 'clients.json'); }
function storedCredentials(host) { try { return JSON.parse(fs.readFileSync(credentialsFile(host), 'utf8')); } catch { return null; } }

const legImplementations = {
  // Request, approve through the console, then a wrong PIN and a cancel.
  async pair(host) {
    const detail = {};
    await host.api('POST', '/api/clients/unpair-all', {}); // an isolated profile: start clean
    // A wrong PIN is refused once and the request is spent.
    let ws = await new Signalling(signalling(host)).open();
    await ws.authRequired();
    ws.send('request-pairing', {pin: '4321', clientName: 'e2e-wrong'});
    await host.approve('e2e-wrong', '1111');
    await assert.rejects(ws.expect('paired'), error => error.code === 'incorrect-pin');
    detail.wrongPin = 'incorrect-pin';
    ws.close();
    // Closing the socket withdraws the request from the console.
    ws = await new Signalling(signalling(host)).open();
    await ws.authRequired();
    ws.send('request-pairing', {pin: '4321', clientName: 'e2e-cancel'});
    for (let i = 0; i < 50 && !(await host.pending()).some(p => p.name === 'e2e-cancel'); i++) await sleep(100);
    assert((await host.pending()).some(p => p.name === 'e2e-cancel'), 'the request never reached the console');
    ws.close();
    for (let i = 0; i < 50 && (await host.pending()).some(p => p.name === 'e2e-cancel'); i++) await sleep(100);
    assert(!(await host.pending()).some(p => p.name === 'e2e-cancel'), 'closing the socket left the request pending');
    detail.cancel = 'withdrawn';
    // The right PIN pairs; the credentials stay in the test profile.
    ws = await new Signalling(signalling(host)).open();
    const paired = await ws.pair('4321', 'e2e-ours', () => host.approve('e2e-ours', '4321'));
    assert.equal(paired.clientId.length, 32); assert.equal(paired.clientSecret.length, 64);
    fs.writeFileSync(credentialsFile(host), JSON.stringify({clientId: paired.clientId, clientSecret: paired.clientSecret}, null, 2));
    await ws.expect('gateway-status'); await ws.expect('capabilities');
    ws.close();
    detail.paired = true;
    return detail;
  },

  async apps(host) {
    const credentials = storedCredentials(host);
    assert(credentials, 'run the pair leg first');
    const ws = await new Signalling(signalling(host)).open();
    const {capabilities} = await ws.authenticate(credentials.clientId, credentials.clientSecret);
    const apps = await ws.apps();
    assert(apps.length > 0, 'no apps');
    const artwork = await ws.artwork(apps[0].id);
    ws.close();
    // A wrong secret is refused.
    const bad = await new Signalling(signalling(host)).open();
    await bad.authRequired();
    bad.send('authenticate', {clientId: credentials.clientId, proof: '0'.repeat(64)});
    await assert.rejects(bad.expect('authenticated'), error => error.code === 'authentication-failed');
    bad.close();
    return {apps: apps.map(a => a.title), artwork: {available: artwork.available, mimeType: artwork.mimeType, bytes: (artwork.data || '').length},
      audioChannels: capabilities.extensions && capabilities.extensions.audioChannels,
      codecs: capabilities.videoModes && capabilities.videoModes[0] && capabilities.videoModes[0].codecs};
  },
};

// ---- streaming ----------------------------------------------------------------

const delta = (last, first, key) => (last?.[key] ?? 0) - (first?.[key] ?? 0);

// What a page's probe measured between two samples, with the host's own view.
function streamResult({first, last, g2g, host, offset, extra}) {
  const elapsed = (last.at - first.at) / 1000;
  const received = delta(last.video, first.video, 'framesReceived'), decoded = delta(last.video, first.video, 'framesDecoded');
  const emitted = delta(last.video, first.video, 'jitterBufferEmittedCount');
  const video = {
    codec: last.video.codec, fmtp: last.video.fmtp, size: `${last.video.frameWidth}x${last.video.frameHeight}`, decoder: last.video.decoderImplementation,
    framesReceived: received, framesDecoded: decoded, framesDropped: delta(last.video, first.video, 'framesDropped'),
    decodedOverReceived: received ? decoded / received : null, decodedFps: decoded / elapsed,
    packetsLost: delta(last.video, first.video, 'packetsLost'), freezeCount: delta(last.video, first.video, 'freezeCount'),
    freezeSeconds: delta(last.video, first.video, 'totalFreezesDuration'),
    jitterBufferDelayMs: emitted ? 1000 * delta(last.video, first.video, 'jitterBufferDelay') / emitted : null,
    decodeMsPerFrame: decoded ? 1000 * delta(last.video, first.video, 'totalDecodeTime') / decoded : null,
    keyFrames: delta(last.video, first.video, 'keyFramesDecoded'), nack: delta(last.video, first.video, 'nackCount'), pli: delta(last.video, first.video, 'pliCount'),
    mbps: 8 * delta(last.video, first.video, 'bytesReceived') / elapsed / 1e6};
  const audio = last.audio ? {
    codec: last.audio.codec, fmtp: last.audio.fmtp, packetsReceived: delta(last.audio, first.audio, 'packetsReceived'),
    packetsLost: delta(last.audio, first.audio, 'packetsLost'), concealedSamples: delta(last.audio, first.audio, 'concealedSamples'),
    totalSamples: delta(last.audio, first.audio, 'totalSamplesReceived')} : null;
  const log = support.hostLog(host.log, offset);
  return {seconds: elapsed, video, audio, glassToGlass: g2g, host: log.summary, hostInput: log.inputs.at(-1) || null, hostWarnings: log.errors, ...extra};
}

// Playwright does not expose the browser's process id; the browser itself knows it.
async function browserPid(browser) {
  const cdp = await browser.newBrowserCDPSession();
  const {processInfo} = await cdp.send('SystemInfo.getProcessInfo');
  return processInfo.find(p => p.type === 'browser')?.id;
}

// Above every other window, so nothing hides the time code from the capture.
function pinClock(clock) {
  execFileSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', path.join(__dirname, 'webrtc_client', 'topmost.ps1'),
    '-Marker', 'rl-e2e-clock', '-ProcessId', String(clock.pid)], {stdio: 'pipe'});
}

async function openClock() {
  const executablePath = support.edgePath();
  assert(executablePath, 'Edge is not installed');
  const browser = await chromium.launch({executablePath, headless: false, args: ['--window-position=0,0', '--window-size=1500,720']});
  const context = await browser.newContext({viewport: null});
  const page = await context.newPage();
  await page.goto(`http://127.0.0.1:${STATIC_PORT}/clock.html`);
  // The window must sit at the top left and in front, or another window clips the time code.
  const cdp = await context.newCDPSession(page);
  const {windowId} = await cdp.send('Browser.getWindowForTarget');
  await cdp.send('Browser.setWindowBounds', {windowId, bounds: {left: 0, top: 0, width: 1500, height: 720, windowState: 'normal'}});
  const pid = await browserPid(browser);
  const clock = {browser, page, pid, forget: onExit(() => killTree(pid))};
  pinClock(clock);
  return clock;
}

async function launchClient(kind, extraArgs = []) {
  const executablePath = kind === 'chrome' ? support.chromePath() : support.edgePath();
  assert(executablePath, `${kind} is not installed`);
  const browser = await chromium.launch({executablePath, headless: true, args: support.clientArgs(extraArgs)});
  const pid = await browserPid(browser);
  const forget = onExit(() => killTree(pid));
  const context = await browser.newContext({viewport: {width: 1280, height: 720}, ignoreHTTPSErrors: true});
  for (const name of ['clock.js', 'probe.js']) await context.addInitScript({content: script(name)});
  return {browser, context, forget};
}

async function saveFrame(page, name) {
  const url = await page.evaluate(() => RlProbe.snapshot(document.getElementById('video'))).catch(() => null);
  if (url) fs.writeFileSync(path.join(output, `frame-${name}.jpg`), Buffer.from(url.split(',')[1], 'base64'));
}

const serverinfo = host => hostSupport.get(`http://127.0.0.1:${host.o.port}/serverinfo`);
const idle = info => /<RustHostSessionCount>0</.test(info) && /<currentgame>0</.test(info) && /<RustHostApplicationActive>0</.test(info);
const wait = ms => new Promise(r => setTimeout(r, ms));

function rumbleProbe(rest) {
  return execFileSync(process.env.BUTTERPOLLO_TEST_PYTHON || 'python', ['-I', path.join(client, 'rumble.py'), ...rest], {encoding: 'utf8'}).trim();
}

// The browser client at its pin: pairs through its own UI, streams H.264
// 1080p60 for `seconds`, announces a gamepad and receives rumble, then stops.
async function browserSession(host) {
  const results = {};
  const staticServer = await support.startStatic(STATIC_PORT);
  const webClient = await support.startWebClient(WEB_PORT);
  const forgetWebClient = onExit(webClient.stop);
  let clock, session;
  try {
    session = await launchClient('edge');
    await session.context.addInitScript({content: script('fakepad.js')});
    const page = await session.context.newPage();
    const pageErrors = [];
    page.on('pageerror', e => pageErrors.push(e.message));
    page.on('console', m => { if (['error', 'warning'].includes(m.type())) pageErrors.push(m.text()); });
    try {
    page.on('dialog', d => d.accept());
    const baseline = JSON.parse(rumbleProbe(['baseline']));
    assert.deepEqual(baseline, [], `XInput pads exist already (${baseline}); not touching them`);
    await page.goto(`${webClient.url}/?host=127.0.0.1&port=${host.o.webrtcPort}`);
    // Pairing through the client's own UI.
    await page.waitForSelector('#pair-pin:visible, button.app', {timeout: 20000});
    if (await page.locator('#pair-pin').isVisible()) {
      const pin = (await page.textContent('#pair-pin')).trim();
      const response = await host.approve(null, pin);
      assert.equal(response.status, 200, response.text);
    }
    await page.waitForSelector('button.app', {timeout: 20000});
    await host.grantAll();
    await page.selectOption('#setting-mode', '1920x1080');
    await page.selectOption('#setting-fps', '60');
    await page.selectOption('#setting-codec', 'h264');
    const settings = await page.evaluate(() => ['setting-mode', 'setting-fps', 'setting-codec', 'setting-bitrate'].map(id => document.getElementById(id).value));
    await hostSupport.assertRealHostIdle();
    // Only now: the clock window sits on top of everything, and nothing before this needs it.
    clock = await openClock();
    const offset = fs.statSync(host.log).size;
    const t0 = Date.now();
    await page.click('button.app >> nth=0');
    await page.waitForFunction(() => {
      const pc = RlProbe.peers.at(-1); return pc && pc.connectionState === 'connected' && document.getElementById('video').videoWidth > 0;
    }, null, {timeout: 60000});
    const startMs = Date.now() - t0;
    await page.waitForFunction(async () => ((await RlProbe.sample()).video || {}).framesDecoded > 30, null, {timeout: 20000, polling: 500});
    await page.evaluate(() => { RlProbe.collect(1000); RlProbe.startG2g(document.getElementById('video')); });
    await wait(4000); // settle: the first seconds hold the keyframe and the encoder ramp
    const first = await page.evaluate(() => RlProbe.sample());
    await page.evaluate(() => { RlProbe.g2g.length = 0; RlProbe.g2gRejected = 0; });
    const gamepad = {};
    try {
      await wait(2000);
      const rumble = JSON.parse(rumbleProbe(['rumble', JSON.stringify(baseline), '1.0']));
      gamepad.xinput = rumble;
      assert(!rumble.error, rumble.error);
      await wait(500);
      gamepad.rumbleEvents = await page.evaluate(() => window.__rumbles);
      assert(gamepad.rumbleEvents.some(e => e.strongMagnitude > 0 || e.weakMagnitude > 0), 'the browser received no rumble');
      gamepad.status = 'passed';
    } catch (error) { gamepad.status = 'failed'; gamepad.error = String(error.message || error); }
    results.gamepad = gamepad;
    await wait(Math.max(0, seconds * 1000 - 7000));
    const last = await page.evaluate(() => RlProbe.sample());
    const g2g = await page.evaluate(() => RlProbe.g2gSummary());
    await saveFrame(page, 'browser-h264');
    const h264 = results.h264 = streamResult({first, last, g2g, host, offset, extra: {settings, startMs, pageErrors}});
    h264.status = h264.video.codec === 'video/H264' && h264.video.decodedOverReceived > .98 ? 'passed' : 'failed';
    // Stop: the client quits the app on the host.
    const stopped = Date.now();
    await page.click('#stream-quit').catch(() => {});
    const stop = results.stop = {};
    for (let i = 0; i < 50 && !stop.status; i++) { if (idle(await serverinfo(host))) stop.status = 'passed'; else await wait(200); }
    stop.status ||= 'failed'; stop.ms = Date.now() - stopped;
    await wait(500);
    h264.hostInput = support.hostLog(host.log, offset).inputs.at(-1) || null;
    } catch (error) {
      error.message += ` [page: ${pageErrors.slice(0, 5).join(' | ')}] [status: ${await page.textContent('#stream-message').catch(() => '')}]`;
      await page.screenshot({path: path.join(output, 'browser-failure.png')}).catch(() => {});
      throw error;
    }
  } finally {
    await session?.browser.close().catch(() => {});
    await clock?.browser.close().catch(() => {});
    session?.forget(); clock?.forget(); forgetWebClient();
    webClient.stop(); staticServer.stop();
  }
  return results;
}

// ---- our own client -------------------------------------------------------------

const MIME = {h264: 'video/H264', hevc: 'video/H265', av1: 'video/AV1'};
const TONE_EXE = process.env.BUTTERPOLLO_TEST_TONE || 'C:\\src\\butterpollo-release\\wr\\target\\release\\examples\\webrtc_tone.exe';

// The sink's mix format, from the tone player: surround needs a sink that
// already has 6 or 8 channels (the harness never changes system audio).
function sinkFormat() {
  return JSON.parse(execFileSync(TONE_EXE, ['--format'], {encoding: 'utf8'}));
}

// The format the sink had before a surround leg changed it, kept until it is put back.
const SINK_RESTORE = path.join(hostSupport.defaults.root, 'sink-restore.json');
function restoreSink(before) {
  execFileSync(TONE_EXE, ['--restore', before]);
  fs.rmSync(SINK_RESTORE, {force: true});
}

// A run that was killed outright (power, taskkill /F) cannot clean up. The next one does it before
// anything else: the sink gets its old format back and the old test host, tone player and browser
// client server are stopped.
function recoverFromEarlierRun() {
  const notes = [];
  if (fs.existsSync(SINK_RESTORE)) {
    try { restoreSink(JSON.parse(fs.readFileSync(SINK_RESTORE, 'utf8')).before); notes.push('restored the sink format an earlier run left'); }
    catch (error) { throw new Error(`${SINK_RESTORE} is there but the sink could not be restored: ${error.message}`); }
  }
  const killed = support.killLeftovers({hostDir: hostSupport.defaults.staging, toneExe: TONE_EXE, webPort: WEB_PORT});
  if (killed.length) notes.push(`stopped leftovers of an earlier run: ${killed.join(', ')}`);
  return notes;
}

const FFPROBE = process.env.BUTTERPOLLO_TEST_FFPROBE || 'ffprobe';

// What the host's bitstream says about itself, read by ffprobe from the encoded frames the client
// kept: the proof that HDR is in the stream and not only in the host's settings. AV1 frames come
// off the wire without temporal delimiters, which the OBU demuxer wants in front of every unit.
function probeBitstream(codec, base64, name, sizes) {
  let data = Buffer.from(base64, 'base64');
  if (codec === 'av1') {
    const units = [];
    let at = 0;
    for (const size of sizes) { units.push(Buffer.from([0x12, 0x00]), data.subarray(at, at + size)); at += size; }
    data = Buffer.concat(units);
  }
  const format = {av1: 'obu', hevc: 'hevc', h264: 'h264'}[codec];
  const file = path.join(output, `${name}.${format}`);
  fs.writeFileSync(file, data);
  const out = JSON.parse(execFileSync(FFPROBE, ['-v', 'error', '-f', format, '-print_format', 'json',
    '-show_streams', '-show_frames', '-select_streams', 'v:0', file], {encoding: 'utf8', maxBuffer: 1 << 28}));
  const stream = out.streams?.[0] || {};
  const sideData = [...new Set((out.frames || []).flatMap(f => (f.side_data_list || []).map(s => s.side_data_type)))];
  return {file, codec: stream.codec_name, profile: stream.profile, pixFmt: stream.pix_fmt, bitsPerRawSample: stream.bits_per_raw_sample,
    colorRange: stream.color_range, colorSpace: stream.color_space, colorTransfer: stream.color_transfer, colorPrimaries: stream.color_primaries,
    size: `${stream.width}x${stream.height}`, frames: (out.frames || []).length, sideData};
}

// An HDR10 stream: 10-bit, BT.2020, PQ and the mastering display metadata. Content light levels
// (MaxCLL, MaxFALL) are 0 from a virtual display, which knows neither, and are not sent; the host
// log's `max_cll` says so.
function hdrProblems(bitstream, configured) {
  const problems = [];
  if (!/10/.test(`${bitstream.pixFmt} ${bitstream.bitsPerRawSample}`)) problems.push(`not 10-bit (${bitstream.pixFmt})`);
  if (bitstream.colorPrimaries !== 'bt2020') problems.push(`primaries ${bitstream.colorPrimaries}`);
  if (bitstream.colorTransfer !== 'smpte2084') problems.push(`transfer ${bitstream.colorTransfer}`);
  if (!/bt2020/.test(bitstream.colorSpace || '')) problems.push(`matrix ${bitstream.colorSpace}`);
  if (!bitstream.sideData.some(s => /Mastering display/.test(s))) problems.push('no mastering display metadata');
  if (String(configured?.hdr) !== 'true' || String(configured?.ten_bit) !== 'true') problems.push(`host log says hdr=${configured?.hdr} ten_bit=${configured?.ten_bit}`);
  return problems;
}

// The monitors Windows has right now: a virtual display that outlives its stream shows up here.
function monitorCount() {
  return Number(execFileSync('powershell.exe', ['-NoProfile', '-Command',
    'Add-Type -AssemblyName System.Windows.Forms; [Windows.Forms.Screen]::AllScreens.Count'], {encoding: 'utf8'}).trim());
}

// Our protocol v2 client (rust/tests/webrtc_client/client.js) streams with a
// forced codec, mode and channel count; the browser is Edge, or Chrome with the
// H.265 receive flag for HEVC. `check` runs against the connected page.
// `clock: false` leaves the glass-to-glass clock out (the capture is not the display the clock
// sits on), `tone: false` skips the audio check, `keyframeMs` asks the host for a keyframe at that
// interval, `bitstream` keeps encoded frames for ffprobe and `refused` expects the host to end the
// session instead of streaming.
async function ourSession(host, {codec, channels = 2, kind = 'edge', browserArgs = [], width = 1920, height = 1080, fps = 60, bitrateKbps = 30000,
  hdr = false, clock: useClock = true, tone: useTone = true, keyframeMs = 0, bitstream = false, refused = false}) {
  const credentials = storedCredentials(host);
  assert(credentials, 'run the pair leg first');
  await host.grantAll();
  const result = {codec, channels, browser: kind, mode: `${width}x${height}@${fps}`, bitrateKbps, hdr};
  const staticServer = await support.startStatic(STATIC_PORT);
  let clock, session, tone, sinkRestore, forgetSink, forgetTone;
  const monitorsBefore = hdr ? monitorCount() : null;
  try {
    session = await launchClient(kind, browserArgs);
    const page = await session.context.newPage();
    const pageErrors = [];
    page.on('pageerror', e => pageErrors.push(e.message));
    page.on('console', m => { if (['error', 'warning'].includes(m.type())) pageErrors.push(m.text()); });
    try {
      await page.goto(`http://127.0.0.1:${STATIC_PORT}/client.html`);
      result.browserVersion = session.browser.version();
      result.receivers = await page.evaluate(() => RTCRtpReceiver.getCapabilities('video').codecs.map(c => c.mimeType).filter((m, i, a) => a.indexOf(m) === i));
      if (!result.receivers.includes(MIME[codec])) {
        result.status = 'not-established';
        result.reason = `${kind} ${result.browserVersion} does not offer a ${MIME[codec]} decoder (${result.receivers.join(', ')})`;
        return result;
      }
      await hostSupport.assertRealHostIdle();
      if (channels > 2) {
        // The Steam sink takes a surround format for the leg and gets its old one back in `finally`.
        // No default device is touched: capture_only keeps the host away from them.
        result.sinkBefore = sinkFormat();
        sinkRestore = JSON.parse(execFileSync(TONE_EXE, ['--set-channels', String(channels)], {encoding: 'utf8'})).before;
        // On disk at once, so a run that dies from here on still gets the sink back at the next start.
        fs.writeFileSync(SINK_RESTORE, JSON.stringify({before: sinkRestore, date: new Date().toISOString()}));
        forgetSink = onExit(() => restoreSink(sinkRestore));
        const format = result.sink = sinkFormat();
        if (format.channels !== channels || !format.float) {
          result.status = 'not-established';
          result.reason = `the capture sink is ${format.channels}-channel and float=${format.float} after asking for ${channels}`;
          return result;
        }
      }
      if (useClock) clock = await openClock();
      const offset = fs.statSync(host.log).size;
      const t0 = Date.now();
      const options = {url: signalling(host), clientId: credentials.clientId, secret: credentials.clientSecret,
        codec, channels, width, height, fps, bitrateKbps, hdr, dumpVideo: bitstream ? 40 : 0};
      if (refused) return await refusedSession(host, page, result, options, offset);
      const started = await page.evaluate(options => RlClient.start(options), options);
      result.startMs = Date.now() - t0;
      Object.assign(result, started);
      await page.waitForFunction(async () => ((await RlProbe.sample()).video || {}).framesDecoded > 30, null, {timeout: 30000, polling: 500});
      await page.evaluate(clocked => { RlProbe.collect(1000); if (clocked) RlProbe.startG2g(document.getElementById('video')); }, useClock);
      if (useTone) {
        tone = spawn(TONE_EXE, [String(seconds + 30)], {stdio: 'ignore', windowsHide: true});
        forgetTone = onExit(() => killTree(tone.pid));
      }
      await wait(4000);
      const first = await page.evaluate(() => RlProbe.sample());
      await page.evaluate(() => { RlProbe.g2g.length = 0; RlProbe.g2gRejected = 0; });
      if (keyframeMs) await page.evaluate(ms => RlClient.requestKeyframes(ms), keyframeMs);
      if (clock) pinClock(clock);
      await wait(seconds * 1000 - 4000);
      if (useTone) result.check = await toneCheck(page, channels);
      await saveFrame(page, `${codec}-${width}x${height}-${fps}${hdr ? '-hdr' : ''}-${channels}ch`);
      const last = await page.evaluate(() => RlProbe.sample());
      const g2g = await page.evaluate(() => RlProbe.g2gSummary());
      if (keyframeMs) result.keyframeRequests = await page.evaluate(() => RlClient.stopKeyframes());
      Object.assign(result, streamResult({first, last, g2g, host, offset, extra: {pageErrors}}));
      result.offers = await page.evaluate(() => ({offer: RlClient.offerSdp.split(/\r?\n/).filter(l => /^a=(rtpmap|fmtp):/.test(l)), answer: RlClient.answerSdp.split(/\r?\n/).filter(l => /^a=(rtpmap|fmtp):/.test(l))}));
      const problems = [];
      if (result.video.codec !== MIME[codec]) problems.push(`codec ${result.video.codec}`);
      if (!(result.video.decodedOverReceived > .98)) problems.push(`decoded ${result.video.decodedOverReceived} of the frames received`);
      // AMD's AV1 encoder pads 1080p by two rows, which the browser reports as 1920x1082
      // (rust/PERFORMANCE.md, AMF AV1 alignment); the sizes measured so far are otherwise exact.
      const padded = codec === 'av1' && height === 1080 ? `${width}x${height + 2}` : null;
      if (![`${width}x${height}`, padded].includes(result.video.size)) problems.push(`size ${result.video.size}`);
      if (useClock && g2g.samples < 50) problems.push(`${g2g.samples} glass-to-glass samples`);
      if (useTone && result.check.status !== 'passed') problems.push('audio check');
      if (bitstream) {
        const dump = await page.evaluate(() => RlClient.videoDump());
        result.dump = {frames: dump.frames, keyFrames: dump.keyFrames};
        const configured = support.fields(support.hostLog(host.log, offset).text.split(/\r?\n/).find(l => l.includes('stream configured')) || '');
        result.streamConfigured = {hdr: configured.hdr, ten_bit: configured.ten_bit, full_range: configured.full_range, color_matrix: configured.color_matrix,
          codec: configured.codec, width: configured.width, height: configured.height, fps: configured.fps, encoder: configured.encoder, source_pixel: configured.source_pixel};
        result.bitstream = probeBitstream(codec, dump.base64, `bitstream-${codec}${hdr ? "-hdr" : ""}-${width}x${height}`, dump.sizes);
        if (hdr) problems.push(...hdrProblems(result.bitstream, configured));
      }
      if (problems.length) result.problems = problems;
      result.status = problems.length ? 'failed' : 'passed';
      // Stop the stream and the app, then wait for the host to go idle.
      const stopped = Date.now();
      await page.evaluate(() => RlClient.stop());
      for (let i = 0; i < 50 && !idle(await serverinfo(host)); i++) await wait(200);
      result.stopMs = Date.now() - stopped; result.stoppedIdle = idle(await serverinfo(host));
      if (!result.stoppedIdle) result.status = 'failed';
      await wait(500);
      result.hostInput = support.hostLog(host.log, offset).inputs.at(-1) || null;
    } catch (error) {
      error.message += ` [page: ${pageErrors.slice(0, 5).join(' | ')}]`;
      await page.screenshot({path: path.join(output, `${codec}-${channels}-failure.png`)}).catch(() => {});
      throw error;
    }
  } finally {
    try { tone?.kill(); } catch {}
    forgetTone?.();
    if (sinkRestore) { try { restoreSink(sinkRestore); forgetSink(); result.sinkRestored = true; } catch (error) { result.sinkRestored = String(error.message); } }
    await session?.browser.close().catch(() => {});
    await clock?.browser.close().catch(() => {});
    session?.forget(); clock?.forget();
    staticServer.stop();
    if (monitorsBefore !== null) {
      // The virtual display goes with its session; a monitor left behind is a failure of the leg.
      for (let i = 0; i < 40 && monitorCount() > monitorsBefore; i++) await wait(500);
      result.monitorsBefore = monitorsBefore; result.monitorsAfter = monitorCount();
      if (result.monitorsAfter > monitorsBefore) { result.status = 'failed'; (result.problems ||= []).push('the virtual display outlived its session'); }
    }
  }
  return result;
}

// `hdr: true` on a display that is not in HDR: the host must say so and end the session, never
// stream SDR as HDR. The leg passes when no video arrives and the host log names the refusal.
async function refusedSession(host, page, result, options, offset) {
  const started = Date.now();
  result.outcome = await page.evaluate(options => RlClient.start(options).then(() => 'connected', e => String(e.message || e)), options);
  let log;
  for (let i = 0; i < 40; i++) {
    log = support.hostLog(host.log, offset);
    if (/HDR was requested but the display is not in HDR/.test(log.text)) break;
    await wait(500);
  }
  result.framesDecoded = await page.evaluate(async () => ((await RlProbe.sample().catch(() => ({}))).video || {}).framesDecoded || 0);
  result.hostRefusal = (log.text.split(/\r?\n/).find(l => /HDR was requested/.test(l)) || '').replace(/^.*?(HDR was requested)/, '$1');
  result.states = await page.evaluate(() => RlClient.states.map(s => `${s.state}: ${s.message || ''}`));
  await page.evaluate(() => RlClient.stop()).catch(() => {});
  for (let i = 0; i < 50 && !idle(await serverinfo(host)); i++) await wait(200);
  result.stoppedIdle = idle(await serverinfo(host));
  const problems = [];
  if (!result.hostRefusal) problems.push('the host log has no HDR refusal');
  if (result.framesDecoded > 0) problems.push(`${result.framesDecoded} frames were decoded`);
  if (!result.stoppedIdle) problems.push('the host is not idle afterwards');
  if (problems.length) result.problems = problems;
  result.status = problems.length ? 'failed' : 'passed';
  result.seconds = (Date.now() - started) / 1000;
  return result;
}

// Each channel of the decoded audio carries its own tone: channel i must peak
// at 500 + 200 * i Hz, well above the floor, or the mapping is wrong.
async function toneCheck(page, channels) {
  const readings = [];
  const tones = Array.from({length: channels}, (_, c) => 500 + 200 * c);
  for (let i = 0; i < 3; i++) { readings.push(await page.evaluate(t => RlProbe.measureChannels(t), tones)); await wait(500); }
  const detail = await page.evaluate(n => ({tracks: RlClient.tracks.audio ? RlClient.tracks.audio.track.getSettings() : null, expected: Array.from({length: n}, (_, c) => RlProbe.toneHz(c))}), channels);
  const verdict = Array.from({length: channels}, (_, c) => {
    const seen = readings.map(r => r[c]);
    const hz = detail.expected[c];
    // Every other channel's tone must sit at least 25 dB below this channel's own.
    const isolationDb = Math.min(...seen.map(s => Math.min(...s.at.filter((_, t) => t !== c).map(level => s.at[c] - level))));
    return {channel: c, expectedHz: hz, peaks: seen.map(s => s.peakHz), levelsDb: seen.map(s => s.peakDb - s.floorDb), isolationDb,
      ok: seen.every(s => Math.abs(s.peakHz - hz) <= 12 && s.peakDb - s.floorDb > 25) && (channels < 2 || isolationDb > 25)};
  });
  if (channels > 2) return surroundCheck(page, channels, detail.tracks, verdict);
  return {status: verdict.every(v => v.ok) ? 'passed' : 'failed', channels: verdict, trackSettings: detail.tracks};
}

// A browser plays remote audio in its output device's layout, so Web Audio only ever sees two
// channels of a 5.1 or 7.1 stream (kept below as `playout`). The verdict comes from the packets
// themselves: the page decodes them with WebCodecs and the answered channel mapping and measures
// every decoded channel for all the tones.
async function surroundCheck(page, channels, tracks, playout) {
  for (let i = 0; i < 30; i++) {
    const n = await page.evaluate(() => (RlClient.decoded?.levels.length ?? 0) + (RlClient.decoded?.errors.length ?? 0));
    if (n >= 4) break;
    await wait(500);
  }
  const decoded = await page.evaluate(() => RlClient.decoded ? ({support: RlClient.decoded.support, errors: RlClient.decoded.errors, decodedChannels: RlClient.decoded.decodedChannels, levels: RlClient.decoded.levels}) : null);
  const windows = (decoded?.levels ?? []).slice(1); // the first one holds the decoder start-up
  const verdict = Array.from({length: channels}, (_, c) => {
    const own = windows.map(w => w[c][c]);
    const other = windows.map(w => Math.max(...w[c].filter((_, t) => t !== c)));
    const isolationDb = Math.min(...own.map((level, i) => level - other[i]));
    // The tone the channel really carries (its index), when it is not its own.
    const last = windows.at(-1)?.[c] ?? [];
    const carries = last.length ? last.indexOf(Math.max(...last)) : null;
    return {channel: c, expectedHz: 500 + 200 * c, carries, ownDbfs: own.slice(-3), otherDbfs: other.slice(-3), isolationDb,
      ok: windows.length >= 3 && own.every(level => level > -40) && isolationDb > 25};
  });
  const ok = verdict.every(v => v.ok) && !decoded?.errors.length && decoded?.decodedChannels === channels;
  return {status: ok ? 'passed' : 'failed', source: 'webcodecs', support: decoded?.support, errors: decoded?.errors, decodedChannels: decoded?.decodedChannels,
    windows: windows.length, channels: verdict, playout: playout.map(v => ({channel: v.channel, peaks: v.peaks, isolationDb: v.isolationDb})), trackSettings: tracks};
}

const H265_RECEIVE = ['--enable-features=WebRtcAllowH265Receive'];
const ourLegs = {
  av1: {codec: 'av1'},
  hevc: {codec: 'hevc', kind: 'chrome', browserArgs: H265_RECEIVE},
  surround51: {codec: 'h264', channels: 6},
  surround71: {codec: 'h264', channels: 8},
  // B5b measurements (docs/webrtc-design.md, rust/PERFORMANCE.md). The 1080p legs keep the clock
  // and the tone; 4K and the frame rates above 60 are measured on the same capture.
  h264: {codec: 'h264'},
  'hevc-4k-idr': {codec: 'hevc', kind: 'chrome', browserArgs: H265_RECEIVE, width: 3840, height: 2160, bitrateKbps: 50000, keyframeMs: 500, tone: false},
  'av1-4k-idr': {codec: 'av1', width: 3840, height: 2160, bitrateKbps: 50000, keyframeMs: 500, tone: false},
  'h264-90': {codec: 'h264', fps: 90},
  'h264-120': {codec: 'h264', fps: 120},
  'av1-90': {codec: 'av1', fps: 90},
  'av1-120': {codec: 'av1', fps: 120},
  'hevc-90': {codec: 'hevc', kind: 'chrome', browserArgs: H265_RECEIVE, fps: 90},
  'hevc-120': {codec: 'hevc', kind: 'chrome', browserArgs: H265_RECEIVE, fps: 120},
  // 90 and 120 fps are offered at every mode the encoder's AMF throughput allows; these two are the
  // largest. A host whose encoder cannot do them refuses the start, and the leg fails loudly.
  'hevc-1440p-120': {codec: 'hevc', kind: 'chrome', browserArgs: H265_RECEIVE, width: 2560, height: 1440, fps: 120, bitrateKbps: 30000, tone: false},
  'hevc-4k-120': {codec: 'hevc', kind: 'chrome', browserArgs: H265_RECEIVE, width: 3840, height: 2160, fps: 120, bitrateKbps: 50000, tone: false},
  'av1-4k-120': {codec: 'av1', width: 3840, height: 2160, fps: 120, bitrateKbps: 50000, tone: false},
  // The same 4K on the virtual display in SDR, to tell the display's cost from HDR's.
  'av1-4k-vd': {codec: 'av1', width: 3840, height: 2160, bitrateKbps: 50000, clock: false, tone: false},
  // HDR needs a virtual display that is in HDR (--system with virtual_display_mode and
  // dd_hdr_option=auto); the clock sits on the physical monitor, so these have no glass-to-glass.
  'hevc-hdr-4k': {codec: 'hevc', kind: 'chrome', browserArgs: H265_RECEIVE, width: 3840, height: 2160, bitrateKbps: 50000, hdr: true, clock: false, tone: false, bitstream: true},
  'av1-hdr-4k': {codec: 'av1', width: 3840, height: 2160, bitrateKbps: 50000, hdr: true, clock: false, tone: false, bitstream: true},
  // With dd_hdr_option=disabled the virtual display is SDR, and the host must refuse hdr:true.
  'hdr-refused': {codec: 'hevc', kind: 'chrome', browserArgs: H265_RECEIVE, width: 3840, height: 2160, bitrateKbps: 50000, hdr: true, clock: false, tone: false, refused: true},
};

(async () => {
  fs.mkdirSync(output, {recursive: true});
  await hostSupport.assertRealHostIdle();
  ctx.notes.push(...recoverFromEarlierRun());
  const settings = hostSupport.prepare({system}, {config: hostConfig});
  if (Object.keys(hostConfig).length) ctx.notes.push(`host config: ${JSON.stringify(hostConfig)}${system ? ', as SYSTEM' : ''}`);
  const host = new hostSupport.TestHost(settings);
  const forgetHost = onExit(() => host.kill());
  let failed = false;
  try {
    await host.start();
    for (const name of legs) {
      for (let run = 1; run <= (ourLegs[name] ? runs : 1); run++) {
        const key = run === 1 ? name : `${name}#${run}`;
        const started = Date.now();
        try {
          if (name === 'browser') Object.assign(ctx.results, await browserSession(host));
          else if (ourLegs[name]) ctx.results[key] = await ourSession(host, ourLegs[name]);
          else ctx.results[name] = {status: 'passed', ...await legImplementations[name](host, ctx)};
        } catch (error) {
          ctx.results[key] = {status: 'failed', error: String(error && error.stack || error)};
        }
        for (const result of Object.values(ctx.results)) { if (!['passed', 'not-established'].includes(result.status)) failed = true; result.seconds ??= (Date.now() - started) / 1000; }
        console.log(key, Object.entries(ctx.results).map(([k, v]) => `${k}:${v.status}`).join(' '));
        if (failed && ctx.results[key]?.status === 'failed' && /exit 3|encoder hang/i.test(JSON.stringify(ctx.results[key]))) throw new Error('an encoder hang: stopping, never retrying');
      }
    }
  } finally {
    await host.stop();
    forgetHost();
    fs.writeFileSync(path.join(output, 'result.json'), JSON.stringify({date: new Date().toISOString(), legs: ctx.results, notes: ctx.notes}, null, 2));
  }
  console.log('result:', path.join(output, 'result.json'));
  process.exit(failed ? 1 : 0);
})().catch(error => { console.error(error); process.exit(2); });
