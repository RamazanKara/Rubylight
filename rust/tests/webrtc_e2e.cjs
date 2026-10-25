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
const WEB_PORT = 48570, STATIC_PORT = 48571;
const client = path.join(__dirname, 'webrtc_client');
const script = name => fs.readFileSync(path.join(client, name), 'utf8');
const stamp = new Date().toISOString().replace(/[-:]/g, '').replace(/\..*/, '');
const output = option('out', process.env.BUTTERPOLLO_TEST_ARTIFACTS || path.join(hostSupport.defaults.root, 'runs', stamp));
const ctx = {results: {}, notes: []};

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
  await page.bringToFront();
  return {browser, page};
}

async function launchClient(kind, extraArgs = []) {
  const executablePath = kind === 'chrome' ? support.chromePath() : support.edgePath();
  assert(executablePath, `${kind} is not installed`);
  const browser = await chromium.launch({executablePath, headless: true, args: support.clientArgs(extraArgs)});
  const context = await browser.newContext({viewport: {width: 1280, height: 720}, ignoreHTTPSErrors: true});
  for (const name of ['clock.js', 'probe.js']) await context.addInitScript({content: script(name)});
  return {browser, context};
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
  let clock, session;
  try {
    clock = await openClock();
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

// Our protocol v2 client (rust/tests/webrtc_client/client.js) streams with a
// forced codec and channel count; the browser is Edge, or Chrome with the
// H.265 receive flag for HEVC. `check` runs against the connected page.
async function ourSession(host, {codec, channels = 2, kind = 'edge', browserArgs = []}) {
  const credentials = storedCredentials(host);
  assert(credentials, 'run the pair leg first');
  await host.grantAll();
  const result = {codec, channels, browser: kind};
  const staticServer = await support.startStatic(STATIC_PORT);
  let clock, session, tone, sinkRestore;
  try {
    clock = await openClock();
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
        const format = result.sink = sinkFormat();
        if (format.channels !== channels || !format.float) {
          result.status = 'not-established';
          result.reason = `the capture sink is ${format.channels}-channel and float=${format.float} after asking for ${channels}`;
          return result;
        }
      }
      const offset = fs.statSync(host.log).size;
      const t0 = Date.now();
      const started = await page.evaluate(options => RlClient.start(options), {
        url: signalling(host), clientId: credentials.clientId, secret: credentials.clientSecret,
        codec, channels, width: 1920, height: 1080, fps: 60, bitrateKbps: 30000});
      result.startMs = Date.now() - t0;
      Object.assign(result, started);
      await page.waitForFunction(async () => ((await RlProbe.sample()).video || {}).framesDecoded > 30, null, {timeout: 30000, polling: 500});
      await page.evaluate(() => { RlProbe.collect(1000); RlProbe.startG2g(document.getElementById('video')); });
      tone = spawn(TONE_EXE, [String(seconds + 30)], {stdio: 'ignore', windowsHide: true});
      await wait(4000);
      const first = await page.evaluate(() => RlProbe.sample());
      await page.evaluate(() => { RlProbe.g2g.length = 0; RlProbe.g2gRejected = 0; });
      await clock.page.bringToFront();
      await wait(seconds * 1000 - 4000);
      result.check = await toneCheck(page, channels);
      await saveFrame(page, `${codec}-${channels}ch`);
      const last = await page.evaluate(() => RlProbe.sample());
      const g2g = await page.evaluate(() => RlProbe.g2gSummary());
      Object.assign(result, streamResult({first, last, g2g, host, offset, extra: {pageErrors}}));
      result.offers = await page.evaluate(() => ({offer: RlClient.offerSdp.split(/\r?\n/).filter(l => /^a=(rtpmap|fmtp):/.test(l)), answer: RlClient.answerSdp.split(/\r?\n/).filter(l => /^a=(rtpmap|fmtp):/.test(l))}));
      const good = result.video.codec === MIME[codec] && result.video.decodedOverReceived > .98 && g2g.samples >= 50 && result.check.status === 'passed';
      result.status = good ? 'passed' : 'failed';
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
    if (sinkRestore) { try { execFileSync(TONE_EXE, ['--restore', sinkRestore]); result.sinkRestored = true; } catch (error) { result.sinkRestored = String(error.message); } }
    await session?.browser.close().catch(() => {});
    await clock?.browser.close().catch(() => {});
    staticServer.stop();
  }
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

const ourLegs = {
  av1: {codec: 'av1'},
  hevc: {codec: 'hevc', kind: 'chrome', browserArgs: ['--enable-features=WebRtcAllowH265Receive']},
  surround51: {codec: 'h264', channels: 6},
  surround71: {codec: 'h264', channels: 8},
};

(async () => {
  fs.mkdirSync(output, {recursive: true});
  await hostSupport.assertRealHostIdle();
  const settings = hostSupport.prepare();
  const host = new hostSupport.TestHost(settings);
  let failed = false;
  try {
    await host.start();
    for (const name of legs) {
      const started = Date.now();
      try {
        if (name === 'browser') Object.assign(ctx.results, await browserSession(host));
        else if (ourLegs[name]) ctx.results[name] = await ourSession(host, ourLegs[name]);
        else ctx.results[name] = {status: 'passed', ...await legImplementations[name](host, ctx)};
      } catch (error) {
        ctx.results[name] = {status: 'failed', error: String(error && error.stack || error)};
      }
      for (const result of Object.values(ctx.results)) { if (!['passed', 'not-established'].includes(result.status)) failed = true; result.seconds ??= (Date.now() - started) / 1000; }
      console.log(name, Object.entries(ctx.results).map(([k, v]) => `${k}:${v.status}`).join(' '));
    }
  } finally {
    await host.stop();
    fs.writeFileSync(path.join(output, 'result.json'), JSON.stringify({date: new Date().toISOString(), legs: ctx.results, notes: ctx.notes}, null, 2));
  }
  console.log('result:', path.join(output, 'result.json'));
  process.exit(failed ? 1 : 0);
})().catch(error => { console.error(error); process.exit(2); });
