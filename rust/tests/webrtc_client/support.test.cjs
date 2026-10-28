// Run with: node --test rust/tests/webrtc_client/support.test.cjs (Windows).
const assert = require('node:assert/strict');
const fs = require('node:fs');
const http = require('node:http');
const os = require('node:os');
const path = require('node:path');
const {test} = require('node:test');
const {spawn} = require('node:child_process');
const support = require('./support.cjs');

const alive = pid => { try { process.kill(pid, 0); return true; } catch { return false; } };
const until = async (condition, ms = 10000) => { for (const end = Date.now() + ms; Date.now() < end; await new Promise(r => setTimeout(r, 100))) if (condition()) return true; return condition(); };

test('leftovers of a killed run are stopped by path and port, and nothing else', async () => {
  const hostDir = fs.mkdtempSync(path.join(os.tmpdir(), 'rl-leftover-'));
  const port = 48599;
  const idle = '-e', forever = 'setInterval(() => {}, 1000)';
  try {
    // A stand-in test host (any exe in the staging directory), a stand-in browser client server on
    // the harness's port, and a bystander in another directory.
    fs.copyFileSync(process.execPath, path.join(hostDir, 'butterpollo.exe'));
    const host = spawn(path.join(hostDir, 'butterpollo.exe'), [idle, forever], {stdio: 'ignore'});
    const server = spawn(process.execPath, [idle, `require('net').createServer().listen(${port}, '127.0.0.1'); ${forever}`], {stdio: 'ignore'});
    const bystander = spawn(process.execPath, [idle, forever], {stdio: 'ignore'});
    await support.waitForPort(port);
    assert(alive(host.pid) && alive(server.pid) && alive(bystander.pid));
    const killed = support.killLeftovers({hostDir, toneExe: path.join(hostDir, 'webrtc_tone.exe'), webPort: port});
    assert(killed.includes('butterpollo.exe') && killed.includes(`port ${port}`), killed.join());
    assert(await until(() => !alive(host.pid) && !alive(server.pid)), 'the leftovers are still running');
    assert(alive(bystander.pid), 'a process outside the staging directory was stopped');
    bystander.kill();
  } finally {
    fs.rmSync(hostDir, {recursive: true, force: true});
  }
});

test('a directory that only shares a prefix with the served one is outside it', () => {
  const dir = path.join(os.tmpdir(), 'webrtc_client');
  assert(support.isInside(dir, path.join(dir, 'clock.html')));
  assert(!support.isInside(dir, path.join(os.tmpdir(), 'webrtc_client_sibling', 'secret.txt')));
  assert(!support.isInside(dir, dir), 'the directory itself is not a file in it');
});

test('the static server serves its files and refuses a directory and a traversal', async () => {
  const server = await support.startStatic(48598);
  const get = route => new Promise((resolve, reject) => http.get(`${server.url}${route}`, response => {
    response.resume(); response.on('end', () => resolve(response.statusCode));
  }).on('error', reject));
  try {
    assert.equal(await get('/clock.html'), 200);
    assert.equal(await get('/..%2Fwebrtc_e2e.cjs'), 404);
    assert.equal(await get('/'), 404, 'the directory itself');
  } finally {
    server.stop();
  }
});

test("the console leg is held to the browser page's part of glass-to-glass, from a frame's arrival", () => {
  const o = {slackMs: 5};
  const browser = {median_ms: 12, arrival_median_ms: 10, wait_median_ms: 2};
  // D4 in the rc.4 pass: 21 ms against 12, with the gap before the frame arrived (host and clock).
  const upstream = support.compareG2g({median_ms: 21, arrival_median_ms: 19, wait_median_ms: 2}, browser, o);
  assert.deepEqual(upstream.problems, []);
  assert.equal(upstream.vsBrowserMs, 9);
  assert.equal(upstream.waitVsBrowserMs, 0);
  assert.match(upstream.notes.join(), /up to the frame's arrival is 9 ms worse .*not the page/);
  // A page that holds each frame for its own work.
  const held = support.compareG2g({median_ms: 21, arrival_median_ms: 10, wait_median_ms: 11}, browser, o);
  assert.match(held.problems.join(), /console page shows a frame 11 ms after it arrived, the browser page 2 ms .*9 ms worse, over 5 ms/);
  assert.deepEqual(held.notes, []);
  assert.deepEqual(support.compareG2g({median_ms: 14, arrival_median_ms: 10, wait_median_ms: 6.5}, browser, o).problems, []);
  // No arrival times on either side is a failure, not a pass on the full medians.
  assert.match(support.compareG2g({median_ms: 12}, browser, o).problems.join(), /from the console leg/);
  assert.match(support.compareG2g({median_ms: 12, arrival_median_ms: 10, wait_median_ms: 2}, {median_ms: 12}, o).problems.join(), /from the browser leg/);
  assert.match(support.compareG2g({median_ms: 12, arrival_median_ms: 10, wait_median_ms: 2}, undefined, o).problems.join(), /from the browser leg/);
});
test('the probe splits each glass-to-glass sample at the frame arrival', () => {
  const root = {};
  require('node:vm').runInNewContext(fs.readFileSync(path.join(__dirname, 'probe.js'), 'utf8'), {globalThis: root});
  const probe = root.RlProbe;
  probe.g2g.push({ms: 20, wait: 2}, {ms: 22, wait: 3}, {ms: 21, wait: 2.5});
  const summary = probe.g2gSummary();
  assert.equal(summary.median_ms, 21);
  assert.equal(summary.wait_median_ms, 2.5);
  assert.equal(summary.arrival_median_ms, 18.5);
  assert.equal(summary.waitSamples, 3);
  // A browser without receiveTime leaves the split empty, which the comparison refuses.
  probe.g2g.length = 0; probe.g2g.push({ms: 20, wait: undefined});
  assert.equal(probe.g2gSummary().arrival_median_ms, null);
});