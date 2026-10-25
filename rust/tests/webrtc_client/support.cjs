// Browsers, static servers and host-log parsing for the WebRTC e2e legs.
const fs = require('node:fs');
const http = require('node:http');
const net = require('node:net');
const path = require('node:path');
const {spawn, execFileSync} = require('node:child_process');

const here = __dirname;
const CLIENT_PIN = '225a7ef54457';
const CLIENT_REPO = 'https://github.com/mlopezsegura/moonlight-webrtc-web.git';
const CLIENT_DIR = path.join(here, '..', 'fixtures', 'webrtc-web');

function firstExisting(candidates) { return candidates.find(p => p && fs.existsSync(p)); }

function edgePath() {
  const core = 'C:\\Program Files (x86)\\Microsoft\\EdgeCore';
  const versions = fs.existsSync(core) ? fs.readdirSync(core).sort().reverse().map(v => path.join(core, v, 'msedge.exe')) : [];
  return firstExisting([process.env.EDGE_EXECUTABLE, 'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe',
    'C:\\Program Files\\Microsoft\\Edge\\Application\\msedge.exe', ...versions]);
}
function chromePath() {
  return firstExisting([process.env.CHROME_EXECUTABLE, 'C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe',
    'C:\\Program Files (x86)\\Google\\Chrome\\Application\\chrome.exe']);
}

// Flags for a client browser: audio muted (the user's headset is never used),
// loopback ICE candidates (the test host listens on 127.0.0.1) and no mDNS.
function clientArgs(extra = []) {
  return ['--autoplay-policy=no-user-gesture-required', '--mute-audio', '--allow-loopback-in-peer-connection',
    '--disable-features=WebRtcHideLocalIpsWithMdns', '--force-webrtc-ip-handling-policy=default', ...extra];
}

function waitForPort(port, timeout = 15000) {
  const deadline = Date.now() + timeout;
  return new Promise((resolve, reject) => {
    const attempt = () => {
      const socket = net.connect(port, '127.0.0.1');
      socket.on('connect', () => { socket.destroy(); resolve(); });
      socket.on('error', () => { socket.destroy(); Date.now() > deadline ? reject(new Error(`nothing listens on ${port}`)) : setTimeout(attempt, 200); });
    };
    attempt();
  });
}

// Fetches the browser client at its pin (never vendored; the directory is
// gitignored) and starts it with its own `npm start`.
async function startWebClient(port) {
  if (!fs.existsSync(path.join(CLIENT_DIR, 'package.json'))) {
    fs.mkdirSync(CLIENT_DIR, {recursive: true});
    const git = (...a) => execFileSync('git', ['-C', CLIENT_DIR, ...a], {stdio: 'pipe'});
    try {
      git('init', '-q'); git('remote', 'add', 'origin', CLIENT_REPO); git('fetch', '-q', 'origin'); git('checkout', '-q', CLIENT_PIN);
    } catch (error) {
      // A half-made checkout has a .git but no package.json, and the next run would trust it.
      fs.rmSync(CLIENT_DIR, {recursive: true, force: true});
      throw error;
    }
  }
  const head = execFileSync('git', ['-C', CLIENT_DIR, 'rev-parse', 'HEAD'], {encoding: 'utf8'}).trim();
  if (!head.startsWith(CLIENT_PIN)) throw new Error(`the browser client is at ${head}, not ${CLIENT_PIN}`);
  const child = spawn(process.platform === 'win32' ? 'npm.cmd' : 'npm', ['start', '--', '--port', String(port), '--host', '127.0.0.1'],
    {cwd: CLIENT_DIR, stdio: 'ignore', windowsHide: true, shell: process.platform === 'win32'});
  await waitForPort(port);
  return {url: `http://127.0.0.1:${port}`, head, stop: () => { try { execFileSync('taskkill', ['/PID', String(child.pid), '/T', '/F'], {stdio: 'ignore'}); } catch {} }};
}

// What a run that was killed outright left behind: its test host, its tone player and the browser
// client's server. Found by executable path and by the harness's own port, so nothing else
// is touched. Returns what it stopped.
function killLeftovers({hostDir, toneExe, webPort}) {
  const quote = text => `'${text.replace(/'/g, "''")}'`;
  const script = `
$ErrorActionPreference = 'SilentlyContinue'
$hostDir = ${quote(hostDir)}; $toneExe = ${quote(toneExe)}; $killed = @()
foreach ($p in Get-CimInstance Win32_Process) {
  $exe = $p.ExecutablePath
  if (-not $exe) { continue }
  if ($exe.StartsWith($hostDir + '\', [StringComparison]::OrdinalIgnoreCase) -or $exe -ieq $toneExe) {
    Stop-Process -Id $p.ProcessId -Force; $killed += $p.Name
  }
}
foreach ($c in Get-NetTCPConnection -LocalPort ${Number(webPort)} -State Listen) {
  taskkill /PID $c.OwningProcess /T /F | Out-Null; $killed += "port ${Number(webPort)}"
}
$killed -join ','`;
  const out = execFileSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', '-Command', script], {encoding: 'utf8'}).trim();
  return out ? out.split(',') : [];
}

// Whether `file` is below `dir`: a sibling that merely starts with the same letters is not.
const isInside = (dir, file) => file.startsWith(dir + path.sep);

// Serves rust/tests/webrtc_client (our client page, the clock page, scripts).
function startStatic(port) {
  const types = {'.html': 'text/html', '.js': 'text/javascript', '.mjs': 'text/javascript', '.css': 'text/css'};
  const server = http.createServer((request, response) => {
    const file = path.join(here, path.normalize(decodeURIComponent(new URL(request.url, 'http://x').pathname)).replace(/^([\\/])+/, ''));
    if (!isInside(here, file) || !fs.existsSync(file) || fs.statSync(file).isDirectory()) { response.writeHead(404); response.end(); return; }
    response.writeHead(200, {'Content-Type': types[path.extname(file)] || 'application/octet-stream', 'Cache-Control': 'no-store'});
    response.end(fs.readFileSync(file));
  });
  return new Promise(resolve => server.listen(port, '127.0.0.1', () => resolve({url: `http://127.0.0.1:${port}`, stop: () => server.close()})));
}

// ---- host log ----------------------------------------------------------------
function fields(line) {
  const out = {};
  for (const [, key, value] of line.matchAll(/\b([a-z_0-9]+)=("[^"]*"|\S+)/g)) {
    const number = Number(value);
    out[key] = Number.isNaN(number) ? value.replace(/^"|"$/g, '') : number;
  }
  return out;
}
const mean = a => a.length ? a.reduce((x, y) => x + y, 0) / a.length : null;
const pct = (a, q) => { if (!a.length) return null; const s = [...a].sort((x, y) => x - y); return s[Math.min(s.length - 1, Math.floor(q * s.length))]; };

// The 5 s "stream timings" lines and the "WebRTC input summary" lines after `offset`.
function hostLog(logPath, offset = 0) {
  const text = fs.readFileSync(logPath, 'utf8').slice(offset);
  const lines = text.split(/\r?\n/);
  const timings = lines.filter(l => l.includes('stream timings')).map(fields);
  const inputs = lines.filter(l => l.includes('WebRTC input summary')).map(fields);
  const column = key => timings.map(t => t[key]).filter(v => typeof v === 'number');
  const summary = {lines: timings.length};
  for (const key of ['fps', 'encode_mean_ms', 'encode_p95_ms', 'present_to_send_mean_ms', 'present_to_send_p99_ms', 'present_to_sent_ms', 'sent_mbps']) {
    const values = column(key); if (values.length) summary[key] = {mean: mean(values), max: Math.max(...values)};
  }
  return {summary, timings, inputs, text, errors: lines.filter(l => /\b(ERROR|WARN)\b/.test(l)).slice(0, 40)};
}

module.exports = {edgePath, chromePath, clientArgs, startWebClient, killLeftovers, isInside, startStatic, hostLog, fields, mean, pct, waitForPort, CLIENT_PIN};
