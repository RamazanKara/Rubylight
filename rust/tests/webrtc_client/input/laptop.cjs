// The 780M laptop's side of the WebRTC input leg: ssh and scp wrappers, the staged test host and
// the echo window, both started in the logged-in user's session through scheduled tasks, and the
// cleanup that leaves nothing behind. Injected input reaches the real desktop, so the host that
// injects it runs here and never on the machine that runs the harness.
'use strict';
const crypto = require('node:crypto');
const fs = require('node:fs');
const http = require('node:http');
const os = require('node:os');
const path = require('node:path');
const {execFileSync} = require('node:child_process');
const {TestHost} = require('../host.cjs');

const SSH = 'C:\\Windows\\System32\\OpenSSH\\ssh.exe';
const SCP = 'C:\\Windows\\System32\\OpenSSH\\scp.exe';
const TARGET = process.env.BUTTERPOLLO_LAPTOP || 'ramaz@192.168.4.12';
const ADDRESS = TARGET.split('@').pop();
const HOSTNAME = process.env.BUTTERPOLLO_LAPTOP_NAME || 'ramazansIdeaPad';
const SSH_OPTS = ['-o', 'BatchMode=yes', '-o', 'ConnectTimeout=10', '-o', 'ServerAliveInterval=15'];

const REMOTE = {
  host: 'C:\\tests\\webrtc-host', config: 'C:\\tests\\webrtc', echo: 'C:\\tests\\webrtc-echo',
  hostTask: 'rl-webrtc-input-host', echoTask: 'rl-webrtc-input-echo', firewall: 'rl-webrtc-input-test',
};
const PORTS = {http: 48140, webrtc: 48545, media: 48546, echo: 48150};
const sleep = ms => new Promise(r => setTimeout(r, ms));
const q = text => `'${String(text).replace(/'/g, "''")}'`;

// Runs a PowerShell 5.1 script on the laptop and returns its stdout.
function remote(script, {timeout = 90000} = {}) {
  const full = `$ProgressPreference='SilentlyContinue'; $ErrorActionPreference='Stop'; ${script}
exit 0`;
  const encoded = Buffer.from(full, 'utf16le').toString('base64');
  let args;
  if (encoded.length < 7000) args = ['powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -EncodedCommand ' + encoded];
  else {
    const file = path.join(os.tmpdir(), 'rl-input-' + crypto.randomBytes(4).toString('hex') + '.ps1');
    fs.writeFileSync(file, full);
    try {
      execFileSync(SSH, [...SSH_OPTS, TARGET, 'powershell -NoProfile -Command "New-Item -ItemType Directory -Force C:\\tests\\rl-ctl | Out-Null"'], {stdio: 'pipe'});
      execFileSync(SCP, [...SSH_OPTS, file, `${TARGET}:C:/tests/rl-ctl/run.ps1`], {stdio: 'pipe'});
    } finally { fs.rmSync(file, {force: true}); }
    args = ['powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -File C:\\tests\\rl-ctl\\run.ps1'];
  }
  try {
    return execFileSync(SSH, [...SSH_OPTS, TARGET, ...args], {encoding: 'utf8', timeout, stdio: ['ignore', 'pipe', 'pipe']}).trim();
  } catch (error) {
    throw new Error(`the laptop command failed (${error.status}): ${String(error.stderr || '').slice(0, 600)} ${String(error.stdout || '').slice(0, 300)}`);
  }
}
const remoteJson = (script, options) => JSON.parse(remote(script, options));

function scp(from, to, {recursive = false} = {}) {
  execFileSync(SCP, [...SSH_OPTS, ...(recursive ? ['-r'] : []), from, to], {stdio: 'pipe', timeout: 180000});
}
const toLaptop = (local, remotePath, options) => scp(local, `${TARGET}:${remotePath.replace(/\\/g, '/')}`, options);
const fromLaptop = (remotePath, local) => scp(`${TARGET}:${remotePath.replace(/\\/g, '/')}`, local);

function httpGet(port, route, timeout = 4000) {
  return new Promise((resolve, reject) => {
    const request = http.get({host: ADDRESS, port, path: route, timeout}, response => {
      let body = ''; response.setEncoding('utf8');
      response.on('data', chunk => body += chunk); response.on('end', () => resolve(body));
    });
    request.on('timeout', () => request.destroy(new Error('timeout'))); request.on('error', reject);
  });
}

class Laptop {
  constructor(root = process.env.BUTTERPOLLO_LAPTOP_HARNESS || 'C:\\tests\\webrtc-laptop') {
    this.root = root; this.address = ADDRESS; this.ports = PORTS; this.remote = REMOTE;
    this.started = {host: false, echo: false, firewall: false};
    this.echoCursor = 0;
  }

  // Refuses to go on unless this really is the laptop, someone is logged in at its console, and the
  // laptop's installed host is idle. A stream or a game is never interrupted.
  verify() {
    const info = remoteJson(`
      $name = $env:COMPUTERNAME
      $user = (Get-CimInstance Win32_ComputerSystem).UserName
      $info = ''
      try { $info = (Invoke-WebRequest http://127.0.0.1:47989/serverinfo -UseBasicParsing -TimeoutSec 5).Content } catch { $info = 'unreachable' }
      $games = @(Get-Process | Where-Object { $_.Path -match 'steamapps|\\\\Games\\\\|Epic Games|\\\\GOG|Battle\\.net' } | ForEach-Object { $_.ProcessName })
      $busy = @(Get-Process butterpollo -ErrorAction SilentlyContinue | Where-Object { $_.Path -notlike 'C:\\tests\\*' } | ForEach-Object { $_.Path })
      @{name=$name; user=$user; info=$info; games=$games; real=$busy} | ConvertTo-Json -Compress`);
    if (String(info.name).toLowerCase() !== HOSTNAME.toLowerCase()) throw new Error(`${TARGET} answers as ${info.name}, not ${HOSTNAME}`);
    if (!info.user) throw new Error('nobody is logged in at the laptop console; not starting');
    const tag = name => (String(info.info).match(new RegExp(`<${name}>([^<]*)</${name}>`)) || [])[1];
    const real = {};
    if (info.info === 'unreachable') real.state = 'not running';
    else {
      for (const name of ['RustHostSessionCount', 'RustHostPendingSessionCount', 'RustHostApplicationActive']) {
        real[name] = tag(name);
        if (tag(name) !== '0') throw new Error(`the laptop's installed host is busy (${name}=${tag(name)}); not starting`);
      }
      if (tag('state') !== 'SUNSHINE_SERVER_FREE') throw new Error("the laptop's installed host is streaming");
      real.state = tag('state');
    }
    if (info.games && info.games.length) throw new Error(`a game appears to be running on the laptop (${[].concat(info.games).join(', ')})`);
    this.user = info.user;
    return {user: info.user, realHost: real, otherHostProcesses: [].concat(info.real || [])};
  }

  stageEcho() {
    remote(`New-Item -ItemType Directory -Force ${q(REMOTE.echo)} | Out-Null`);
    toLaptop(path.join(__dirname, 'echo.ps1'), REMOTE.echo + '\\echo.ps1');
  }

  // The test exe, its libraries and the console assets, copied from the newest build.
  stage(binary, packageDir) {
    remote(`New-Item -ItemType Directory -Force ${q(REMOTE.host)}, ${q(REMOTE.config + '\\logs')}, ${q(REMOTE.echo)} | Out-Null; ` +
      `Remove-Item -Recurse -Force ${q(REMOTE.host + '\\assets')} -ErrorAction SilentlyContinue; New-Item -ItemType Directory -Force ${q(REMOTE.host + '\\assets')} | Out-Null`);
    toLaptop(binary, REMOTE.host + '\\butterpollo.exe');
    for (const name of fs.readdirSync(packageDir)) if (name.endsWith('.dll')) toLaptop(path.join(packageDir, name), REMOTE.host + '\\' + name);
    toLaptop(path.join(packageDir, 'assets', 'web'), REMOTE.host + '\\assets\\', {recursive: true});
    toLaptop(path.join(__dirname, 'echo.ps1'), REMOTE.echo + '\\echo.ps1');
    return remote(`(Get-Item ${q(REMOTE.host + '\\butterpollo.exe')}).Length`);
  }

  // The isolated profile and the generated console login. The password stays in harness.json on this
  // PC and in the laptop profile's state file; it is never printed.
  profile(extra = {}) {
    fs.mkdirSync(this.root, {recursive: true});
    const file = path.join(this.root, 'harness.json');
    let harness = fs.existsSync(file) ? JSON.parse(fs.readFileSync(file, 'utf8')) : null;
    if (!harness) {
      harness = {username: 'test', password: crypto.randomBytes(18).toString('base64url'), salt: crypto.randomBytes(8).toString('hex')};
      fs.writeFileSync(file, JSON.stringify(harness, null, 2));
    }
    const digest = crypto.createHash('sha256').update(harness.password + harness.salt).digest();
    const lines = {
      port: PORTS.http, encoder: 'amf', capture: 'wgc', minimum_fps_target: 60,
      virtual_display_mode: 'disabled', virtual_display_layout: 'extended',
      dd_configuration_option: 'disabled', dd_resolution_option: 'disabled', dd_refresh_rate_option: 'disabled', dd_hdr_option: 'disabled',
      dd_always_restore_from_golden: false, dd_config_revert_on_disconnect: false, frame_limiter_enable: false,
      install_steam_audio_drivers: false, stream_audio: true, audio_sink_capture_only: true, auto_capture_sink: false,
      keep_sink_default: false, upnp: false, enable_discovery: false, vulkan_hdr_layer: false, system_tray: false,
      update_check_interval: 0, pyrowave: false,
      webrtc_server: true, webrtc_port: PORTS.webrtc, webrtc_media_port: PORTS.media, ...extra,
    };
    const dir = path.join(this.root, 'profile');
    fs.mkdirSync(dir, {recursive: true});
    fs.writeFileSync(path.join(dir, 'sunshine.conf'), Object.entries(lines).map(([k, v]) => `${k} = ${v}`).join('\n') + '\n');
    fs.writeFileSync(path.join(dir, 'apps.json'), JSON.stringify({apps: [{name: 'Desktop', cmd: '', 'virtual-display': false}]}));
    fs.writeFileSync(path.join(dir, 'sunshine_state.json'), JSON.stringify({
      username: harness.username, salt: harness.salt, password: Buffer.from(digest).reverse().toString('hex').toUpperCase()}));
    for (const name of ['sunshine.conf', 'apps.json', 'sunshine_state.json']) toLaptop(path.join(dir, name), REMOTE.config + '\\' + name);
    return {...harness, conf: lines};
  }

  firewall(on) {
    if (on) {
      remote(`Get-NetFirewallRule -DisplayName ${q(REMOTE.firewall)} -ErrorAction SilentlyContinue | Remove-NetFirewallRule; ` +
        `New-NetFirewallRule -DisplayName ${q(REMOTE.firewall)} -Direction Inbound -Action Allow -Profile Any -Protocol TCP -LocalPort ${PORTS.http},${PORTS.http + 1},${PORTS.webrtc},${PORTS.echo} | Out-Null; ` +
        `New-NetFirewallRule -DisplayName ${q(REMOTE.firewall + '-udp')} -Direction Inbound -Action Allow -Profile Any -Protocol UDP -LocalPort ${PORTS.media} | Out-Null; ` +
        `New-NetFirewallRule -DisplayName ${q(REMOTE.firewall + '-exe')} -Direction Inbound -Action Allow -Profile Any -Program ${q(REMOTE.host + '\\butterpollo.exe')} | Out-Null`);
      this.started.firewall = true;
    } else {
      remote(`Get-NetFirewallRule -DisplayName ${q(REMOTE.firewall + '*')} -ErrorAction SilentlyContinue | Remove-NetFirewallRule`);
      this.started.firewall = false;
    }
  }

  // Starts `exe` in the logged-in user's interactive session through a one-shot scheduled task.
  startTask(name, exe, argument, directory) {
    this.started[name === REMOTE.echoTask ? 'echo' : 'host'] = true;
    remote(`$user = (Get-CimInstance Win32_ComputerSystem).UserName; ` +
      `Unregister-ScheduledTask -TaskName ${q(name)} -Confirm:$false -ErrorAction SilentlyContinue; ` +
      `$action = New-ScheduledTaskAction -Execute ${q(exe)} -Argument ${q(argument)} -WorkingDirectory ${q(directory)}; ` +
      `$principal = New-ScheduledTaskPrincipal -UserId $user -LogonType Interactive; ` +
      `$settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit (New-TimeSpan -Seconds 0); ` +
      `Register-ScheduledTask -TaskName ${q(name)} -Action $action -Principal $principal -Settings $settings -Force | Out-Null; ` +
      `Start-ScheduledTask -TaskName ${q(name)}`);
  }

  async startEcho() {
    this.echoLog = REMOTE.echo + '\\echo.jsonl';
    remote(`Remove-Item -Force ${q(this.echoLog)} -ErrorAction SilentlyContinue`);
    this.startTask(REMOTE.echoTask, 'powershell.exe',
      `-STA -NoProfile -WindowStyle Hidden -ExecutionPolicy Bypass -File ${REMOTE.echo}\\echo.ps1 -Log ${this.echoLog} -Port ${PORTS.echo}`, REMOTE.echo);
    const deadline = Date.now() + 40000;
    for (;;) {
      try { const state = JSON.parse(await httpGet(PORTS.echo, '/state')); if (state.lines > 0) break; } catch {}
      if (Date.now() > deadline) throw new Error('the echo window did not come up on the laptop');
      await sleep(500);
    }
    this.echoCursor = 0;
    const ready = (await this.echoLines()).find(e => e.ev === 'ready');
    if (!ready) throw new Error('the echo window logged no ready line');
    return ready;
  }

  // New echo lines since the last call (or from `from`).
  async echoLines(from = this.echoCursor) {
    const text = await httpGet(PORTS.echo, '/lines?from=' + from);
    const lines = text.split('\n').filter(Boolean).map(l => JSON.parse(l));
    if (from === this.echoCursor) this.echoCursor += lines.length;
    return lines;
  }
  async echoCount() { return JSON.parse(await httpGet(PORTS.echo, '/state')).lines; }
  async echoState() { return JSON.parse(await httpGet(PORTS.echo, '/state')); }

  async startHost() {
    remote(`Remove-Item -Force ${q(REMOTE.config + '\\logs\\butterpollo.log')} -ErrorAction SilentlyContinue`);
    this.startTask(REMOTE.hostTask, REMOTE.host + '\\butterpollo.exe',
      `--config-dir ${REMOTE.config} --assets ${REMOTE.host}\\assets\\web --port ${PORTS.http} --bind ${ADDRESS} --no-tray`, REMOTE.host);
  }

  hostLogText() {
    try { return remote(`Get-Content -Raw -Encoding UTF8 ${q(REMOTE.config + '\\logs\\butterpollo.log')}`); } catch { return ''; }
  }
  async waitReady(timeout = 90000) {
    const deadline = Date.now() + timeout;
    while (!/encoder capability probe completed/.test(this.hostLogText())) {
      if (Date.now() > deadline) throw new Error('the laptop test host did not become ready');
      await sleep(2000);
    }
  }

  // Copies the host log and the echo log back to `dir`.
  fetchLogs(dir) {
    fs.mkdirSync(dir, {recursive: true});
    const out = {};
    try { fromLaptop(REMOTE.config + '\\logs\\butterpollo.log', path.join(dir, 'laptop-host.log')); out.host = path.join(dir, 'laptop-host.log'); } catch (error) { out.hostError = error.message; }
    try { fromLaptop(this.echoLog || REMOTE.echo + '\\echo.jsonl', path.join(dir, 'echo.jsonl')); out.echo = path.join(dir, 'echo.jsonl'); } catch (error) { out.echoError = error.message; }
    return out;
  }

  // Stops the echo window and the test host, removes the tasks and firewall rules, and reports what
  // is still running. Safe to call twice and after any failure.
  async cleanup() {
    const notes = [];
    const step = async (what, fn) => { try { await fn(); } catch (error) { notes.push(`${what}: ${String(error.message).slice(0, 200)}`); } };
    await step('echo stop', async () => { await httpGet(PORTS.echo, '/stop', 3000); await sleep(800); });
    await step('tasks', async () => remote(
      `foreach ($n in ${q(REMOTE.hostTask)}, ${q(REMOTE.echoTask)}) { Stop-ScheduledTask -TaskName $n -ErrorAction SilentlyContinue; Unregister-ScheduledTask -TaskName $n -Confirm:$false -ErrorAction SilentlyContinue }`));
    await step('processes', async () => remote(
      `Get-Process butterpollo -ErrorAction SilentlyContinue | Where-Object { $_.Path -like 'C:\\tests\\*' } | Stop-Process -Force; ` +
      `Get-CimInstance Win32_Process -Filter "Name='powershell.exe'" | Where-Object { $_.CommandLine -like '*webrtc-echo*echo.ps1*' } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force }`));
    await step('firewall', async () => this.firewall(false));
    let left = null;
    await step('check', async () => {
      left = remoteJson(`@{procs=@(Get-Process butterpollo -ErrorAction SilentlyContinue | Where-Object { $_.Path -like 'C:\\tests\\*' } | ForEach-Object { $_.Id }); ` +
        `echo=@(Get-CimInstance Win32_Process -Filter "Name='powershell.exe'" | Where-Object { $_.CommandLine -like '*echo.ps1*' } | ForEach-Object { $_.ProcessId }); ` +
        `tasks=@(Get-ScheduledTask -TaskName 'rl-webrtc-input-*' -ErrorAction SilentlyContinue | ForEach-Object { $_.TaskName }); ` +
        `rules=@(Get-NetFirewallRule -DisplayName 'rl-webrtc-input-test*' -ErrorAction SilentlyContinue | ForEach-Object { $_.DisplayName }); ` +
        `real=@(Get-Process butterpollo -ErrorAction SilentlyContinue | Where-Object { $_.Path -notlike 'C:\\tests\\*' } | ForEach-Object { $_.Path })} | ConvertTo-Json -Compress`);
    });
    return {notes, left};
  }
}

// The laptop's test host as the harness's usual TestHost: its console API is reached over the LAN
// and its log is copied from the laptop.
class RemoteHost extends TestHost {
  constructor(laptop, options) { super({...options, address: ADDRESS}); this.laptop = laptop; }
  get log() { return path.join(this.laptop.root, 'laptop-host.log'); }
  pullLog() { fromLaptop(REMOTE.config + '\\logs\\butterpollo.log', this.log); return this.log; }
  async start() { await this.laptop.startHost(); await this.laptop.waitReady(); await this.login(); }
  async stop() { try { await this.api('POST', '/api/quit', {}); } catch {} }
}

module.exports = {Laptop, RemoteHost, remote, remoteJson, httpGet, ADDRESS, PORTS, REMOTE, TARGET};
