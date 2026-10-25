// Keyboard, mouse and gamepad-as-mouse end-to-end leg for the WebRTC server (docs/webrtc-design.md,
// "Testing"). Injected input reaches the real desktop, so the host under test runs on the 780M
// laptop and an echo window there logs what arrives; this PC runs only the browser.
//
//   node rust/tests/webrtc_input_e2e.cjs [--out <dir>] [--binary <exe>] [--package <dir>] [--no-stage]
//
// Needs PLAYWRIGHT_MODULE (playwright-core) like webrtc_e2e.cjs, ssh access to the laptop
// (BUTTERPOLLO_LAPTOP, default ramaz@192.168.4.12) and its console user logged in. Writes result.json
// and the laptop's echo and host logs to --out. Exit 0 on pass, 1 on a failed check, 2 on an error.
'use strict';
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const {Signalling} = require('./webrtc_client/signalling.js');
const hostSupport = require('./webrtc_client/host.cjs');
const support = require('./webrtc_client/support.cjs');
const {Laptop, RemoteHost, ADDRESS, PORTS} = require('./webrtc_client/input/laptop.cjs');
const plan = require('./webrtc_client/input/plan.cjs');
const {chromium} = require(process.env.PLAYWRIGHT_MODULE || 'playwright');

const args = process.argv.slice(2);
const option = (name, fallback) => { const i = args.indexOf('--' + name); return i >= 0 ? args[i + 1] : fallback; };
const STATIC_PORT = 48580;
const client = path.join(__dirname, 'webrtc_client');
const script = name => fs.readFileSync(path.join(client, name), 'utf8');
const sleep = ms => new Promise(r => setTimeout(r, ms));
const stamp = new Date().toISOString().replace(/[-:]/g, '').replace(/\..*/, '');
const laptop = new Laptop();
const output = option('out', path.join(laptop.root, 'runs', stamp));
const result = {started: new Date().toISOString(), phases: {}, checks: [], notes: []};
const SETTLE = 900;

const record = (phase, checks) => {
  result.phases[phase] = checks;
  for (const check of checks) result.checks.push({phase, ...check});
  for (const check of checks) console.log(`${check.ok ? 'PASS' : 'FAIL'}  ${phase}: ${check.name}${check.ok ? '' : ' ' + JSON.stringify(check).slice(0, 400)}`);
};

async function main() {
  fs.mkdirSync(output, {recursive: true});
  await hostSupport.assertRealHostIdle(); // this PC's own host: the browser decodes a stream here
  result.laptop = laptop.verify();
  console.log('laptop ok:', JSON.stringify(result.laptop.realHost));
  const credentialsFile = path.join(laptop.root, 'clients.json');
  const binary = option('binary', hostSupport.defaults.binary);
  const packageDir = option('package', hostSupport.defaults.package);
  if (!args.includes('--no-stage')) result.stagedBytes = laptop.stage(binary, packageDir); else laptop.stageEcho();
  const harness = laptop.profile();
  laptop.firewall(true);
  const echoReady = await laptop.startEcho();
  result.echo = {screens: echoReady.screens, window: echoReady.window};
  const host = new RemoteHost(laptop, {...hostSupport.defaults, ...harness, port: PORTS.http, webrtcPort: PORTS.webrtc, mediaPort: PORTS.media});
  await host.start();

  // ---- pair and authenticate ---------------------------------------------------------------
  await host.api('POST', '/api/clients/unpair-all', {});
  const signalling = `ws://${ADDRESS}:${PORTS.webrtc}`;
  const pairing = await new Signalling(signalling).open();
  const paired = await pairing.pair('4321', 'input-leg', () => host.approve('input-leg', '4321'));
  assert.equal(paired.clientId.length, 32); assert.equal(paired.clientSecret.length, 64);
  fs.writeFileSync(credentialsFile, JSON.stringify({clientId: paired.clientId, clientSecret: paired.clientSecret}, null, 2));
  pairing.close();
  await host.grantAll();

  // ---- the browser on this PC ------------------------------------------------------------------
  const staticServer = await support.startStatic(STATIC_PORT);
  const executablePath = support.edgePath();
  assert(executablePath, 'Edge is not installed');
  const browser = await chromium.launch({executablePath, headless: true, args: support.clientArgs()});
  try {
    const context = await browser.newContext({viewport: {width: 1280, height: 720}, ignoreHTTPSErrors: true});
    for (const name of ['probe.js', 'input/input.js']) await context.addInitScript({content: script(name)});
    const page = await context.newPage();
    const pageErrors = [];
    page.on('pageerror', e => pageErrors.push(e.message));
    page.on('console', m => { if (['error', 'warning'].includes(m.type())) pageErrors.push(m.text()); });
    result.pageErrors = pageErrors;
    try {
      await page.goto(`http://127.0.0.1:${STATIC_PORT}/client.html`);
      laptop.verify();
      fs.rmSync(host.log, {force: true}); // the log pulled by an earlier run
      const offset = 0;
      const t0 = Date.now();
      const credentials = JSON.parse(fs.readFileSync(credentialsFile, 'utf8'));
      const started = await page.evaluate(options => RlClient.start(options), {
        url: signalling, clientId: credentials.clientId, secret: credentials.clientSecret,
        codec: 'h264', channels: 2, width: 1920, height: 1080, fps: 60, bitrateKbps: 30000});
      result.stream = {startMs: Date.now() - t0, ...started};
      await page.waitForFunction(async () => ((await RlProbe.sample()).video || {}).framesDecoded > 30, null, {timeout: 30000, polling: 500});
      let sample;
      for (let i = 0; i < 20 && !(sample && sample.frameWidth); i++) { if (i) await sleep(300); sample = (await page.evaluate(() => RlProbe.sample())).video; }
      const stream = [sample.frameWidth, sample.frameHeight];
      Object.assign(result.stream, {codec: sample.codec, size: stream.join('x')});
      assert.equal(stream.join('x'), '1920x1080', 'the stream is not 1920x1080');
      console.log(`streaming ${sample.codec} ${stream.join('x')}`);

      const ev = (fn, arg) => page.evaluate(fn, arg);
      const ensureFocus = async () => {
        for (let i = 0; i < 20; i++) { if ((await laptop.echoState()).focus) return; await sleep(250); }
        throw new Error('the echo window lost focus on the laptop; not sending input');
      };
      // Runs `fn`, waits for the input to land, and returns the echo lines it produced.
      const phase = async (name, fn) => {
        await ensureFocus();
        const from = await laptop.echoCount();
        const extra = await fn();
        await sleep(SETTLE);
        return {events: await laptop.echoLines(from), extra, from};
      };
      const run = steps => ev(s => RlInput.run(s), steps);
      const hold = (state, ms) => ev(([s, m]) => RlInput.hold(s, m), [state, ms]);
      const tapPad = async (bits, ms = 150) => { await hold({buttons: bits}, ms); await hold({buttons: 0}, 100); };

      // ---- the input channel ----------------------------------------------------------------
      result.inputChannel = await ev(() => RlInput.open());
      record('channel', [result.inputChannel.ordered && result.inputChannel.maxRetransmits === null && result.inputChannel.maxPacketLifeTime === null
        ? plan.pass('the input channel is ordered and reliable', result.inputChannel) : plan.fail('the input channel is ordered and reliable', result.inputChannel)]);

      // ---- keys ---------------------------------------------------------------------------------
      const keys = plan.keySteps();
      result.layout = echoReady.layout;
      let r = await phase('keys', () => run(keys));
      record('keys', plan.checkKeys(r.events, keys, echoReady.layout));

      // ---- text ---------------------------------------------------------------------------------
      r = await phase('text', () => run(plan.textSteps()));
      record('text', plan.checkText(r.events));

      // ---- relative moves, from the middle of the screen ---------------------------------------------
      await run(plan.centreStep());
      await sleep(300);
      r = await phase('relative', () => run(plan.relativeSteps()));
      record('relative', plan.checkRelative(r.events));

      // ---- absolute positions -----------------------------------------------------------------------
      r = await phase('absolute', () => run(plan.absoluteSteps()));
      record('absolute', plan.checkAbsolute(r.events, echoReady.screens, stream));

      // ---- buttons and wheel (at the middle, over the echo window) ----------------------------------------
      await run(plan.centreStep());
      await sleep(300);
      r = await phase('buttons', () => run(plan.buttonSteps()));
      record('buttons', plan.checkButtons(r.events));
      r = await phase('wheel', () => run(plan.wheelSteps()));
      record('wheel', plan.checkWheel(r.events));

      // ---- stuck keys: the channel closes with keys and a button down -----------------------------------
      r = await phase('stuck', async () => {
        await run(plan.stuckSteps());
        await sleep(300);
        result.stuckDownSeen = (await laptop.echoLines(await laptop.echoCount() - 12)).filter(e => e.src === 'll').length;
        result.closeState = await ev(() => RlInput.close());
        await sleep(1500);
      });
      record('stuck', plan.checkStuck(r.events));

      // The host takes a new input channel after the old one went away.
      result.reopened = await ev(() => RlInput.open());
      r = await phase('reopen', () => run([...plan.tap('KeyZ'), 30]));
      record('reopen', [plan.ll(r.events, 'kd').some(e => e.vk === plan.vkOf('KeyZ', echoReady.layout)) && plan.ll(r.events, 'ku').some(e => e.vk === plan.vkOf('KeyZ', echoReady.layout))
        ? plan.pass('a new input channel works after the old one closed', {}) : plan.fail('a new input channel works after the old one closed', {events: r.events.length})]);

      // ---- gamepad mouse mode ---------------------------------------------------------------------------
      const control = () => ev(() => RlInput.controlMessages());
      const modeMessages = async () => (await control()).filter(m => m.type === 'mouse-mode');
      const waitMode = async (active, timeout = 3000) => {
        const end = Date.now() + timeout;
        while (Date.now() < end) { const last = (await modeMessages()).at(-1); if (last && last.active === active) return last; await sleep(100); }
        return null;
      };
      const channels = await ev(() => RlInput.channelsOpen());
      record('gamepad', [channels.control === 'open' && channels.gamepad === 'open' ? plan.pass('the host opened the control and gamepad channels', channels) : plan.fail('the host opened the control and gamepad channels', channels)]);
      await run(plan.centreStep());
      await sleep(300);
      await ev(() => RlInput.announce(0));
      await sleep(800);

      // Start held 900 ms then released turns mouse mode on.
      const before = (await modeMessages()).length;
      await hold({buttons: plan.PAD.START}, 900);
      await hold({buttons: 0}, 100);
      const on = await waitMode(true);
      record('mouse-mode', [on ? plan.pass('holding Start 750 ms turns mouse mode on and the client gets mouse-mode', {message: on, messages: (await modeMessages()).length - before})
        : plan.fail('holding Start 750 ms turns mouse mode on and the client gets mouse-mode', {control: (await control()).slice(-6)})]);
      if (on) record('mouse-mode', [on.v === 2 && on.controllerId === 0 && on.active === true ? plan.pass('the mouse-mode message is v 2 for controller 0', on) : plan.fail('the mouse-mode message is v 2 for controller 0', on)]);

      if (on) {
        // The stick drives the pointer: the same delta every 50 ms tick, in each direction.
        const moved = [];
        for (const move of plan.PAD_MOVES) {
          r = await phase('pad-' + move.name, async () => { await hold(move.state, 700); await hold({}, 100); });
          moved.push(plan.checkPadMove(move.name, r.events, move.delta));
        }
        // The pointer actually travelled (a final position check on the whole walk).
        record('mouse-mode', moved);

        // A, B, X, LB, RB click; the d-pad scrolls.
        r = await phase('pad-clicks', async () => { for (const [button] of plan.PAD_CLICKS) await tapPad(plan.PAD[button]); });
        record('mouse-mode', [plan.checkPadClicks(r.events)]);
        r = await phase('pad-notches', async () => { for (const [button] of plan.PAD_NOTCHES) await tapPad(plan.PAD[button]); });
        record('mouse-mode', [plan.checkPadNotches(r.events)]);

        // Leave mouse mode with A held: the simulated left button must be released by leaving.
        r = await phase('pad-leave', async () => {
          await hold({buttons: plan.PAD.A}, 300);
          await hold({buttons: plan.PAD.A | plan.PAD.START}, 900);
          await hold({buttons: plan.PAD.A}, 100);
          await sleep(500);
          result.afterLeave = (await laptop.echoLines(await laptop.echoCount() - 6)).filter(e => e.src === 'll').map(e => [e.ev, e.button || null]);
          await hold({buttons: plan.PAD.A}, 400);
          await hold({buttons: 0}, 100);
        });
        const downs = plan.ll(r.events, 'bd'), ups = plan.ll(r.events, 'bu');
        const off = await waitMode(false, 2000);
        record('mouse-mode', [
          off ? plan.pass('holding Start again turns mouse mode off and the client gets mouse-mode active false', {message: off})
            : plan.fail('holding Start again turns mouse mode off and the client gets mouse-mode active false', {control: (await control()).slice(-6)}),
          downs.length === 1 && ups.length === 1 && downs[0].button === 'left' && ups[0].button === 'left'
            ? plan.pass('leaving mouse mode releases every simulated button, and A clicks nothing after', {events: r.events.filter(e => e.src === 'll').map(e => [e.ev, e.button])})
            : plan.fail('leaving mouse mode releases every simulated button, and A clicks nothing after', {downs: downs.length, ups: ups.length, events: r.events.filter(e => e.src === 'll').map(e => [e.ev, e.button])}),
        ]);
      }
      await ev(() => RlInput.unplug(0));
      await sleep(500);

      // ---- end the stream and read the host's own view ---------------------------------------------------
      result.controlMessages = (await control()).map(m => ({type: m.type, active: m.active, controllerId: m.controllerId}));
      await ev(() => RlClient.stop());
      let summary = null;
      for (let i = 0; i < 40 && !summary; i++) {
        await sleep(500);
        host.pullLog();
        summary = support.hostLog(host.log, offset).inputs.at(-1) || null;
      }
      result.hostInput = summary;
      if (summary) console.log(`WebRTC input summary: queue_max_us=${summary.queue_max_us} queue_p99_us=${summary.queue_p99_us}`);
      else result.notes.push('the host logged no "WebRTC input summary" line');
      result.hostWarnings = support.hostLog(host.log, offset).errors;
    } catch (error) {
      error.message += ` [page: ${pageErrors.slice(0, 5).join(' | ')}]`;
      await page.screenshot({path: path.join(output, 'failure.png')}).catch(() => {});
      throw error;
    }
  } finally {
    await browser.close().catch(() => {});
    staticServer.stop();
    await host.stop();
  }
}

(async () => {
  let code = 0;
  try { await main(); } catch (error) { result.error = String(error.stack || error).slice(0, 1500); console.log('ERROR', error.message); code = 2; }
  try {
    result.cleanup = await laptop.cleanup();
    result.logs = laptop.fetchLogs(output);
  } catch (error) { result.cleanup = {error: error.message}; code = code || 2; }
  const failed = result.checks.filter(c => !c.ok);
  result.passed = result.checks.length - failed.length; result.failed = failed.length;
  result.status = code === 0 && !failed.length && result.checks.length ? 'passed' : 'failed';
  fs.writeFileSync(path.join(output, 'result.json'), JSON.stringify(result, null, 2));
  console.log(`${result.status}: ${result.passed} passed, ${result.failed} failed; ${path.join(output, 'result.json')}`);
  console.log('cleanup:', JSON.stringify(result.cleanup && (result.cleanup.left || result.cleanup)));
  process.exit(code || (failed.length ? 1 : 0));
})();
