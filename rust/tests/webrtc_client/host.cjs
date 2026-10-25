// The isolated test host for the WebRTC end-to-end harness: profile, process
// and the console API that approves pairing requests. Nothing here touches the
// installed host except the read-only idle check.
const crypto = require('node:crypto');
const fs = require('node:fs');
const https = require('node:https');
const http = require('node:http');
const path = require('node:path');
const {spawn, execFileSync} = require('node:child_process');

const REAL_SERVERINFO = 'http://127.0.0.1:47989/serverinfo';
const REAL_LOG = 'C:\\ProgramData\\Butterpollo\\config\\logs\\butterpollo.log';

function get(url, timeout = 3000) {
  return new Promise((resolve, reject) => {
    const request = http.get(url, {timeout}, response => {
      let body = ''; response.setEncoding('utf8');
      response.on('data', chunk => body += chunk); response.on('end', () => resolve(body));
    });
    request.on('timeout', () => request.destroy(new Error('timeout'))); request.on('error', reject);
  });
}

// Throws unless the installed host is idle: no session, nothing pending, no
// app, and no game running. A stream is never started on top of the user.
async function assertRealHostIdle() {
  const info = await get(REAL_SERVERINFO);
  const value = tag => (info.match(new RegExp(`<${tag}>([^<]*)</${tag}>`)) || [])[1];
  for (const tag of ['RustHostSessionCount', 'RustHostPendingSessionCount', 'RustHostApplicationActive']) {
    if (value(tag) !== '0') throw new Error(`the installed host is busy (${tag}=${value(tag)}); not starting a stream`);
  }
  if (value('state') !== 'SUNSHINE_SERVER_FREE') throw new Error('the installed host is streaming');
  const games = execFileSync('powershell', ['-NoProfile', '-Command',
    "Get-Process | Where-Object { $_.Path -match 'steamapps|\\\\Games\\\\|Epic Games|\\\\GOG|Battle\\.net' } | Select-Object -ExpandProperty ProcessName"], {encoding: 'utf8'}).trim();
  if (games) throw new Error(`a game appears to be running (${games.split(/\s+/).join(', ')}); not starting a stream`);
  return true;
}

const defaults = {
  root: 'C:\\tests\\webrtc',
  staging: 'C:\\tests\\webrtc-host',
  // The release build, its codec libraries and the console assets.
  binary: process.env.BUTTERPOLLO_TEST_BINARY || 'C:\\src\\butterpollo-release\\wr\\target\\release\\butterpollo.exe',
  package: process.env.BUTTERPOLLO_TEST_PACKAGE || 'C:\\src\\butterpollo-release\\dc\\pkg',
  port: 48140, webrtcPort: 48545, mediaPort: 48546,
};

// Writes the profile on first use and returns the harness settings, with the
// generated console password and sink. The password stays in harness.json.
function prepare(options = {}, {config = {}} = {}) {
  const o = {...defaults, ...options};
  // Audio goes to Steam Streaming Speakers, captured only: the host never
  // changes the system's default devices, so the user's headset stays theirs.
  const probe = process.env.BUTTERPOLLO_TEST_AUDIO_PROBE || 'C:\\src\\butterpollo-release\\fixtures\\audio_probe.exe';
  const sink = JSON.parse(execFileSync(probe, ['--list'], {encoding: 'utf8'})).find(e => e.virtual_sink);
  if (!sink) throw new Error('Steam Streaming Speakers is required for the isolated audio test');
  o.sink = sink.id;
  fs.mkdirSync(path.join(o.root, 'logs'), {recursive: true});
  const harnessFile = path.join(o.root, 'harness.json');
  let harness = fs.existsSync(harnessFile) ? JSON.parse(fs.readFileSync(harnessFile, 'utf8')) : null;
  if (!harness) {
    harness = {username: 'test', password: crypto.randomBytes(18).toString('base64url'), salt: crypto.randomBytes(8).toString('hex')};
    fs.writeFileSync(harnessFile, JSON.stringify(harness, null, 2));
  }
  const digest = crypto.createHash('sha256').update(harness.password + harness.salt).digest();
  // The state file also holds the paired clients: write it once, then keep it.
  const state = path.join(o.root, 'sunshine_state.json');
  if (!fs.existsSync(state)) fs.writeFileSync(state, JSON.stringify({
    username: harness.username, salt: harness.salt, password: Buffer.from(digest).reverse().toString('hex').toUpperCase()}));
  const lines = {
    port: o.port, encoder: 'amf', capture: 'wgc', minimum_fps_target: 60,
    virtual_display_mode: 'disabled', virtual_display_layout: 'extended',
    dd_configuration_option: 'disabled', dd_resolution_option: 'disabled', dd_refresh_rate_option: 'disabled', dd_hdr_option: 'disabled',
    dd_always_restore_from_golden: false, dd_config_revert_on_disconnect: false, frame_limiter_enable: false,
    install_steam_audio_drivers: false, stream_audio: true, audio_sink_capture_only: true, auto_capture_sink: false,
    keep_sink_default: false, upnp: false, enable_discovery: false, vulkan_hdr_layer: false, system_tray: false,
    update_check_interval: 0, pyrowave: false,
    webrtc_server: true, webrtc_port: o.webrtcPort, webrtc_media_port: o.mediaPort,
    audio_sink: o.sink, ...config,
  };
  fs.writeFileSync(path.join(o.root, 'sunshine.conf'), Object.entries(lines).map(([k, v]) => `${k} = ${v}`).join('\n') + '\n');
  fs.writeFileSync(path.join(o.root, 'apps.json'), JSON.stringify({apps: [{name: 'Desktop', cmd: '', 'virtual-display': false}]}));
  return {...o, ...harness};
}

function stage(o) {
  fs.mkdirSync(o.staging, {recursive: true});
  fs.copyFileSync(o.binary, path.join(o.staging, 'butterpollo.exe'));
  for (const name of fs.readdirSync(o.package)) if (name.endsWith('.dll')) fs.copyFileSync(path.join(o.package, name), path.join(o.staging, name));
  return path.join(o.staging, 'butterpollo.exe');
}

class TestHost {
  constructor(o) { this.o = o; this.cookies = ''; this.csrf = ''; }
  get log() { return path.join(this.o.root, 'logs', 'butterpollo.log'); }
  readLog() { return fs.existsSync(this.log) ? fs.readFileSync(this.log, 'utf8') : ''; }

  async start() {
    const exe = stage(this.o);
    fs.writeFileSync(this.log, '');
    this.child = spawn(exe, ['--config-dir', this.o.root, '--assets', path.join(this.o.package, 'assets', 'web'),
      '--port', String(this.o.port), '--bind', '127.0.0.1', '--no-tray'], {
      env: {...process.env, RUST_LOG: process.env.BUTTERPOLLO_E2E_RUST_LOG || 'info', NO_PROXY: '*', PATH: `${path.dirname(exe)};${process.env.PATH}`},
      stdio: 'ignore', windowsHide: true});
    this.exited = new Promise(resolve => this.child.on('exit', code => resolve(code)));
    const deadline = Date.now() + 60000;
    while (!/encoder capability probe completed/.test(this.readLog())) {
      if (this.child.exitCode !== null) throw new Error(`the test host exited with ${this.child.exitCode}`);
      if (Date.now() > deadline) throw new Error('the test host did not become ready');
      await new Promise(r => setTimeout(r, 250));
    }
    await this.login();
  }

  async stop() {
    if (!this.child || this.child.exitCode !== null) return;
    try { await this.api('POST', '/api/quit', {}); } catch {}
    const code = await Promise.race([this.exited, new Promise(r => setTimeout(() => r(null), 8000))]);
    if (code === null) { try { execFileSync('taskkill', ['/PID', String(this.child.pid), '/T', '/F'], {stdio: 'ignore'}); } catch {} }
  }

  request(method, route, body, headers = {}) {
    const web = `https://127.0.0.1:${this.o.port + 1}`;
    return new Promise((resolve, reject) => {
      const data = body === undefined ? undefined : JSON.stringify(body);
      const request = https.request(web + route, {method, rejectUnauthorized: false, headers: {
        ...(data ? {'Content-Type': 'application/json', 'Content-Length': Buffer.byteLength(data)} : {}),
        ...(this.cookies ? {Cookie: this.cookies} : {}), ...(this.csrf ? {'X-CSRF-Token': this.csrf} : {}), ...headers}}, response => {
        let text = ''; response.setEncoding('utf8');
        response.on('data', chunk => text += chunk);
        response.on('end', () => {
          for (const cookie of response.headers['set-cookie'] || []) {
            const pair = cookie.split(';')[0]; const name = pair.split('=')[0];
            this.cookies = this.cookies.split('; ').filter(c => c && !c.startsWith(name + '=')).concat(pair).join('; ');
          }
          let json; try { json = JSON.parse(text); } catch {}
          resolve({status: response.statusCode, json, text});
        });
      });
      request.on('error', reject); if (data) request.write(data); request.end();
    });
  }
  async api(method, route, body) {
    const response = await this.request(method, route, body);
    if (response.status >= 400) throw new Error(`${method} ${route} ${response.status} ${response.text.slice(0, 200)}`);
    return response.json;
  }
  async login() {
    const login = await this.api('POST', '/api/auth/login', {username: this.o.username, password: this.o.password});
    this.csrf = login.csrf_token;
  }
  // A device paired after the first one may only watch until the console
  // grants more (docs/webrtc-design.md); the harness grants everything.
  async grantAll() {
    for (const c of (await this.api('GET', '/api/clients/list')).clients || []) {
      if ((c.perm & 0x071f1f00) !== 0x071f1f00) await this.api('POST', '/api/clients/update', {uuid: c.uuid, perm: 0x071f1f00});
    }
  }
  pending() { return this.api('GET', '/api/clients/pending').then(r => r.requests || []); }
  // Approves the pending request that has `name`, with the PIN the client shows.
  async approve(name, pin) {
    const deadline = Date.now() + 10000;
    for (;;) {
      const match = (await this.pending()).find(p => name === null || p.name === name);
      if (match) return this.request('POST', '/api/pin', {pin, name, uniqueid: match.uniqueid});
      if (Date.now() > deadline) throw new Error(`no pending pairing request from ${name}`);
      await new Promise(r => setTimeout(r, 200));
    }
  }
}

module.exports = {assertRealHostIdle, prepare, TestHost, defaults, REAL_LOG, get};
