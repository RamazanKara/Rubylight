// End-to-end legs for the WebRTC server (docs/webrtc-design.md, "Testing").
// Starts an isolated test host (C:\tests\webrtc, ports 48123 and 48545+),
// pairs, lists apps and streams to real browsers. Never touches the installed
// host except to read its /serverinfo: every run starts with that idle check.
//
//   node rust/tests/webrtc_e2e.cjs [--legs pair,apps,...] [--seconds 60]
//
// Requires Playwright (PLAYWRIGHT_MODULE), Edge or Chrome and Windows.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const {Signalling} = require('./webrtc_client/signalling.js');
const hostSupport = require('./webrtc_client/host.cjs');

const args = process.argv.slice(2);
const option = (name, fallback) => { const i = args.indexOf('--' + name); return i >= 0 ? args[i + 1] : fallback; };
const ALL_LEGS = ['pair', 'apps'];
const legs = option('legs', ALL_LEGS.join(',')).split(',');
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

(async () => {
  fs.mkdirSync(output, {recursive: true});
  await hostSupport.assertRealHostIdle();
  const settings = hostSupport.prepare({}, {});
  const host = new hostSupport.TestHost(settings);
  let failed = false;
  try {
    await host.start();
    for (const name of legs) {
      const started = Date.now();
      try {
        ctx.results[name] = {status: 'passed', ...await legImplementations[name](host, ctx)};
      } catch (error) {
        failed = true;
        ctx.results[name] = {status: 'failed', error: String(error && error.stack || error)};
      }
      ctx.results[name].seconds = (Date.now() - started) / 1000;
      console.log(name, ctx.results[name].status);
    }
  } finally {
    await host.stop();
    fs.writeFileSync(path.join(output, 'result.json'), JSON.stringify({date: new Date().toISOString(), legs: ctx.results, notes: ctx.notes}, null, 2));
  }
  console.log('result:', path.join(output, 'result.json'));
  process.exit(failed ? 1 : 0);
})().catch(error => { console.error(error); process.exit(2); });
