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
