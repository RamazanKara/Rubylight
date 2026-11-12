// Console screenshots against fixture data, for design review. Never contacts a host.
//   PLAYWRIGHT_MODULE=<path to playwright> node scripts/screenshots.cjs <out-dir> [label]
// Writes <label>-<page>-<theme>-<form>.png for each page, theme and form factor.
// SHOT_PAGES=/,/devices picks pages (/design.html is the component gallery);
// SHOT_FULL=1 captures the full page on desktop too (phones always are).
// SHOT_CLICK=Select,"Select all" clicks those buttons, in order, before each shot.
// SHOT_KEYS=Control+k presses those keys, in order, after the clicks; SHOT_TYPE=text then types it
// (Control+k and SHOT_TYPE=po show the quick switcher with results).
// SHOT_AUTH=signed-out shows the sign-in page, SHOT_AUTH=setup first-run setup
// (the fixture is signed in otherwise); either one shoots that page by default.
// SHOT_COOP=missing, failed, blocked or notice sets Nucleus Co-op's folder state (/troubleshoot, /library/steam-620).
// SHOT_CONFIG='{"amd_usage":"auto"}' adds saved settings to the fixture's configuration.
// SHOT_HOME sets Home's state: fresh (nothing paired), fresh-off (and pairing off), warning (paired, audio and
// wake need a look), ready, streaming, or paired-now (a device pairs a few seconds in; with SHOT_WAIT=7000).
// Each gives Portal 2 a last-played time and the home art as its cover. SHOT_WAIT=ms waits before each shot.
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || 'playwright');
const path = require('node:path');
const fs = require('node:fs');

const out = path.resolve(process.argv[2] || 'shots');
const label = process.argv[3] || 'shot';
const auth = process.env.SHOT_AUTH || '';
if (auth && !['signed-out', 'setup'].includes(auth)) throw new Error(`SHOT_AUTH is signed-out or setup, not ${auth}`);
const pages = (process.env.SHOT_PAGES || { 'signed-out': '/login', setup: '/setup' }[auth] || '/,/devices,/settings,/help').split(',');
const themes = ['light', 'dark'];
const forms = { desktop: { width: 1440, height: 900 }, phone: { width: 390, height: 844 } };

const metadata = {
  version: '3.0.0-rc.3', host_name: 'Gaming PC', pc_address: '192.168.4.20', pc_addresses: ['192.168.4.20'],
  paired_devices: 2, enable_pairing: false, warnings: [],
  encoder_status: { state: 'ready', h264: true, hevc: true, av1: true, pyrowave: true },
  capture_status: { displays: [{ display_name: 'DISPLAY1', adapter: 'AMD Radeon RX 7900 XT' }], virtual_display_configured: true },
  gpus: [{ name: 'AMD Radeon RX 7900 XT', vendor: 0x1002, dedicated_memory: 20 << 30, luid: [1, 0], pnp_id: null }],
  virtual_display: { capable: true, ready: true }, audio_enabled: true, features: {},
};
const replies = {
  '/api/auth/status': {
    status: true, credentials_configured: auth !== 'setup', authenticated: !auth,
    ...(auth === 'signed-out' ? { creds_program: 'C:\\Program Files\\Rubylight\\butterpollo.exe' } : {}),
  },
  '/api/csrf-token': { csrf_token: 'fixture-token' },
  '/api/config': { sunshine_name: 'Gaming PC', enable_pairing: false, ...JSON.parse(process.env.SHOT_CONFIG || '{}') },
  '/api/metadata': metadata,
  '/api/clients/pending': { requests: [] },
  '/api/clients/list': { clients: [
    { uuid: 'phone', name: 'Pixel 9', enabled: true, connected: true, perm: 0x071f1f00 },
    { uuid: 'tv', name: 'Living room TV', enabled: true, connected: false, perm: 0x071f1f00 },
  ] },
  '/api/clients/hdr-profiles': { profiles: [] },
  '/api/display-devices': [],
  '/api/apps': { apps: [
    { uuid: 'desktop', name: 'Desktop' }, { uuid: 'steam', name: 'Steam Big Picture', cmd: 'steam://open/bigpicture' },
    { uuid: 'steam-620', name: 'Portal 2', 'steam-id': '620', 'synced-by': 'steam' },
    { uuid: 'mirage', name: 'Assassin’s Creed Mirage', 'launcher-source': 'ubisoft', 'launcher-id': '13504', 'synced-by': 'launcher' },
    { uuid: 'fortnite', name: 'Fortnite', 'launcher-source': 'epic', 'launcher-id': 'Fortnite', 'synced-by': 'launcher' },
    { uuid: 'forza', name: 'Forza Horizon 5', 'launcher-source': 'microsoft', 'launcher-id': 'Microsoft.SunriseBaseGame', 'synced-by': 'launcher' },
    { uuid: 'diablo', name: 'Diablo IV', 'launcher-source': 'battlenet', 'launcher-id': 'Fen', 'synced-by': 'launcher' },
    { uuid: 'hades', name: 'Hades II', 'playnite-id': 'b7c1', 'synced-by': 'playnite' },
    { uuid: 'witcher', name: 'The Witcher 3: Wild Hunt', 'launcher-source': 'gog', 'launcher-id': '1207664663', 'synced-by': 'launcher', 'not-installed': true },
  ], 'launcher-dismissed': ['steam:546560', 'steam:617830'] },
  '/api/library/discovery': { status: true,
    sources: [['steam', 'Steam', 'available'], ['epic', 'Epic Games', 'available'], ['battlenet', 'Battle.net', 'available'],
      ['microsoft', 'Microsoft Store and Xbox', 'available'], ['ea', 'EA app', 'unavailable'], ['ubisoft', 'Ubisoft Connect', 'available'],
      ['gog', 'GOG', 'error'], ['local', 'Game folders', 'disabled']]
      .map(([source, name, state]) => ({ source, name, state, errors: state === 'error' ? ['GOG Galaxy’s game list could not be read.'] : [] })),
    games: [
      ['steam', '620', 'Portal 2', true], ['steam', '546560', 'Half-Life: Alyx', false], ['steam', '617830', 'SUPERHOT VR', false],
      ['steam', '1145360', 'Hades', false], ['ubisoft', '13504', 'Assassin’s Creed Mirage', true], ['epic', 'Fortnite', 'Fortnite', true],
      ['epic', 'Kinglet', 'Alan Wake 2', false], ['microsoft', 'Microsoft.SunriseBaseGame', 'Forza Horizon 5', true],
      ['battlenet', 'Fen', 'Diablo IV', true], ['battlenet', 'Pro', 'Overwatch 2', false],
    ].map(([source, id, name, already_added]) => ({ source, id, uuid: `${source}-${id}`, name, install_dir: '', already_added,
      unavailable_reason: null, has_cover: false })) },
  '/api/playnite/status': { status: true, enabled: true, active: true, available: true, installed: true, installed_version: '1.4.0',
    packaged_version: '1.4.0', update_available: false, auto_sync: true, game_count: 42, synced_seconds_ago: 30 },
  '/api/session/status': { activeSessions: 0, appRunning: false },
  '/api/rtsp/sessions': { status: true, sessions: [] },
  '/api/logs/tail': { offset: 0, text: '', reset: false },
  '/api/updates': { releases: [], checked_at: 0, auto_update: true, update_check_interval: 0,
    install_supported: true, update_available: false, phase: 'idle' },
  '/api/auth/sessions': { sessions: [] },
  '/api/tokens': { tokens: [] },
  '/api/token/routes': { status: true, routes: [
    { path: '/api/apps', methods: ['GET', 'POST'] }, { path: '/api/session/status', methods: ['GET'] },
    { path: '/api/clients/list', methods: ['GET'] }, { path: '/api/logs/tail', methods: ['GET'] },
  ] },
  '/api/usb': { status: true, devices: [] },
  '/api/webhooks': { status: true, max: 8, error: null,
    events: ['pairing.paired', 'pairing.unpaired', 'app.started', 'app.stopped', 'session.started', 'session.ended'],
    hooks: [{ url: 'https://home.example/hooks/rubylight', events: ['app.started', 'app.stopped'],
      last: { event: 'app.started', delivery: 'fixture', time_unix_ms: Date.now() - 12 * 60_000, ok: true, http_status: 200, error: null, attempts: 1 } }] },
  // Nucleus Co-op in the folder the user chose (rust/core/src/nucleus/install.rs VERSION and RELEASE_PAGE);
  // SHOT_COOP=missing shows it not set up yet, SHOT_COOP=failed a folder with another release, SHOT_COOP=blocked
  // one Windows still marks as downloaded, SHOT_COOP=notice another release kept, so the earlier install runs.
  '/api/coop': { mode: { setting: 'nucleus', mode: 'nucleus', label: 'Nucleus Co-op', reason: '' },
    install: { release: { version: '2.4.2', page: 'https://github.com/SplitScreen-Me/splitscreenme-nucleus/releases/tag/v2.4.2' }, ...{
      configured: { state: 'configured', path: 'C:\\Program Files\\Nucleus Co-op', chosen: 'C:\\Program Files\\Nucleus Co-op',
        version: '2.4.2', problem: null, error: null },
      missing: { state: 'missing', path: null, chosen: '', version: null, problem: null, error: null },
      failed: { state: 'failed', path: null, chosen: 'C:\\Program Files\\Nucleus Co-op', version: null, problem: 'version',
        error: 'The NucleusCoop.exe in C:\\Program Files\\Nucleus Co-op isn\'t Nucleus Co-op 2.4.2\'s: download 2.4.2 from its release page and choose its folder' },
      blocked: { state: 'failed', path: null, chosen: 'C:\\Program Files\\Nucleus Co-op', version: null, problem: 'blocked',
        error: 'C:\\Program Files\\Nucleus Co-op\\NucleusCoop.exe is marked as downloaded from the internet. Unblock the zip before extracting: right-click NucleusApp.zip, choose Properties, tick Unblock, then unpack it again into a new folder' },
      notice: { state: 'configured', path: 'C:\\ProgramData\\Butterpollo\\nucleus\\2.4.2', chosen: 'C:\\Program Files\\Nucleus Co-op',
        version: '2.4.2', problem: null, error: null,
        notice: 'The NucleusCoop.exe in C:\\Program Files\\Nucleus Co-op isn\'t Nucleus Co-op 2.4.2\'s: download 2.4.2 from its release page and choose its folder. Until then Rubylight runs the Nucleus Co-op 2.4.2 an earlier Rubylight installed, in C:\\ProgramData\\Butterpollo\\nucleus\\2.4.2' },
    }[process.env.SHOT_COOP || 'configured'] },
    available: true, reason: null, elevates: true, admin_protection: false, open_refusal: null, opened: false,
    games: [{ app: 'steam-620', name: 'Portal 2', status: 'unknown', reason: null, handler: null, players: null, entry: 'Portal 2 (Co-op)' }],
    catalog_error: null, sessions: [] },
  '/api/logs/analysis': { status: true, findings: [], since: '2026-10-31T08:02:11Z', until: '2026-10-31T09:41:57Z', read_bytes: 812_544 },
  '/api/session/console': { status: true, recovery: { session_id: 1, console_session_id: 1, can_reattach: false,
    reason: 'Rubylight already runs on the physical console.' } },
  '/api/display/golden_status': { status: true, exists: true, comparison_available: true, current_mismatch_reason: '' },
  '/api/frame-limiter/status': { enabled: false, configured_provider: 'none', active_provider: 'none', resolved_path: '' },
  '/api/power': power(),
};

// SHOT_HOME: Home's states, on top of the fixture above.
const home = process.env.SHOT_HOME || '';
const HOMES = ['fresh', 'fresh-off', 'warning', 'ready', 'streaming', 'paired-now'];
if (home && !HOMES.includes(home)) throw new Error(`SHOT_HOME is one of ${HOMES.join(', ')}, not ${home}`);
/** Requests for the device list so far: paired-now pairs Pixel 9 after the first. */
let clientLists = 0;
const pixel = { uuid: 'phone', name: 'Pixel 9', enabled: true, connected: false, perm: 0x071f1f00 };
if (home) {
  const portal = replies['/api/apps'].apps.find(app => app.uuid === 'steam-620');
  Object.assign(portal, { 'image-path': 'covers/portal-2.png', 'last-played': Math.floor(Date.now() / 1000) - 2 * 86400 });
}
if (home.startsWith('fresh') || home === 'paired-now') {
  Object.assign(metadata, { paired_devices: 0, enable_pairing: home !== 'fresh-off' });
  replies['/api/clients/list'] = { clients: [] };
}
if (['ready', 'streaming', 'paired-now'].includes(home)) {
  metadata.audio_sinks = [{ id: 'speakers', name: 'Speakers', description: 'Realtek Audio', adapter: '', default: true, virtual_sink: false }];
  replies['/api/power'] = power(true);
}
if (home === 'streaming') {
  replies['/api/session/status'] = { activeSessions: 1, appRunning: true, appName: 'Portal 2', paused: false, running: true,
    app: { name: 'Portal 2', id: 3, uuid: 'steam-620' } };
  const history = Array.from({ length: 90 }, (_, second) => ({ fps: 119 + (second % 3 === 0 ? 1 : 0),
    bitrate_mbps: 44 + 6 * Math.sin(second / 7) + (second % 11 === 0 ? 5 : 0), encode_mean_ms: 2.9, encode_max_ms: 4.1,
    host_processing_mean_ms: 3.2, host_processing_max_ms: 4.4, frame_age_mean_ms: 0.8 }));
  replies['/api/rtsp/sessions'] = { status: true, sessions: [{
    uuid: 'phone', device_name: 'Pixel 9', role: 'stream', state: 'RUNNING', width: 2400, height: 1080, fps: 120,
    video_format: 1, hdr: false, vrr: false, encoder: 'amf', encoder_bitrate_kbps: 50_000, pyrowave_minimum_kbps: null,
    pyrowave_recommended_kbps: null, audio_channels: 2, frames_sent: 10_800, bytes_sent: 0, idr_requests: 1,
    uptime_seconds: 754, warnings: [],
    performance: { fps: 120, bitrate_mbps: 47.6, encode_mean_ms: 2.9, encode_p95_ms: 3.4, encode_p99_ms: 3.8,
      host_processing_mean_ms: 3.2, host_processing_p95_ms: 3.7, host_processing_p99_ms: 4.1, host_processing_max_ms: 4.4,
      frame_age_mean_ms: 0.8, frame_age_p95_ms: 1.2, present_to_send_mean_ms: 4.1, send_interval_p99_ms: 9, sample_frames: 240, history },
  }] };
}
/** Answers that change while a page is open. */
function dynamic(pathname) {
  if (home !== 'paired-now') return undefined;
  if (pathname === '/api/clients/list') return { clients: ++clientLists > 1 ? [pixel] : [] };
  if (pathname === '/api/metadata') return { ...metadata, paired_devices: clientLists > 1 ? 1 : 0 };
  return undefined;
}
/** A pairing QR code's picture that encodes nothing: three finder squares and a fixed fill. */
function sampleQr(size = 29) {
  let seed = 7, cells = '';
  const finder = (x, y) => x >= 0 && y >= 0 && x < 7 && y < 7 && (x % 6 === 0 || y % 6 === 0 || (x > 1 && x < 5 && y > 1 && y < 5));
  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      seed = (Math.imul(seed, 1103515245) + 12345) & 0x7fffffff;
      const corner = [[0, 0], [size - 7, 0], [0, size - 7]].find(([cx, cy]) => x - cx >= -1 && x - cx < 8 && y - cy >= -1 && y - cy < 8);
      if (corner ? finder(x - corner[0], y - corner[1]) : (seed >>> 16) % 2) cells += `M${x} ${y}h1v1h-1z`;
    }
  }
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="-2 -2 ${size + 4} ${size + 4}" shape-rendering="crispEdges"><path d="${cells}"/></svg>`;
}
/** The wake status: an adapter that needs a setting turned on, or (`ready`) one that wakes the PC. */
function power(ready = false) {
  return {
    status: true,
    wake: { revision: 1, primary: '9c:6b:00:12:34:56', targets: [
      { mac: '9c:6b:00:12:34:56', kind: 'ethernet', connected: true, route: true, broadcasts: ['192.168.4.255'], state: ready ? 'ready' : 'needs_setup',
        checks: [{ id: 'wake_enabled', state: ready ? 'ok' : 'error' }, { id: 'magic_packet', state: 'ok' }, { id: 'magic_packet_only', state: ready ? 'ok' : 'warning' }] },
      { mac: '9c:6b:00:ab:cd:ef', kind: 'wifi', connected: false, route: false, broadcasts: [], state: 'experimental', checks: [] },
    ], system: { sleep: 's3', modern_standby: false, hibernate: true, fast_startup: 'active', shutdown_wake: 'firmware_unknown' } },
    idle_sleep: { state: 'armed', minutes: 30, reason: 'waiting for console activity', remaining_seconds: 1260 },
    last_sleep: null,
    fix: { running: false, changes: ready ? [] : ['Realtek PCIe 2.5GbE: allow this device to wake the computer',
      'Realtek PCIe 2.5GbE: only allow a magic packet to wake the computer'], skipped: [], error: null,
    fast_startup_off_offered: true, last: null },
  };
}

(async () => {
  fs.mkdirSync(out, { recursive: true });
  const { createServer } = await import('vite');
  const { svelte } = await import('@sveltejs/vite-plugin-svelte');
  const server = await createServer({ root: path.join(__dirname, '..'), configFile: false, plugins: [svelte()],
    server: { host: '127.0.0.1', port: 0, hmr: false } });
  await server.listen();
  const base = `http://127.0.0.1:${server.httpServer.address().port}`;
  const browser = await chromium.launch({ headless: true,
    ...(process.env.CHROMIUM_EXECUTABLE ? { executablePath: process.env.CHROMIUM_EXECUTABLE } : {}) });
  try {
    for (const [form, viewport] of Object.entries(forms)) {
      for (const theme of themes) {
        const page = await browser.newPage({ viewport, colorScheme: theme });
        await page.route('**/*', async route => {
          const url = new URL(route.request().url());
          if (url.origin !== base) return route.abort();
          if (!url.pathname.startsWith('/api/')) return route.continue();
          if (route.request().method() === 'POST' && url.pathname === '/api/pairing/qr') {
            return route.fulfill({ json: { status: true, codes: [{ address: metadata.pc_address, svg: sampleQr(), uri: 'rubylight://pair?fixture' }],
              name: metadata.host_name, expires_in_seconds: 180 } });
          }
          if (route.request().method() !== 'GET') return route.fulfill({ json: { status: true } });
          if (home && url.pathname === '/api/apps/steam-620/cover') {
            return route.fulfill({ path: path.join(__dirname, '..', 'src', 'assets', 'home-landscape.webp'), contentType: 'image/webp' });
          }
          return route.fulfill({ json: dynamic(url.pathname) ?? replies[url.pathname] ?? { status: true, available: false, enabled: false } });
        });
        for (const where of pages) {
          clientLists = 0;
          await page.goto(base + where);
          await page.waitForLoadState('networkidle');
          await page.waitForTimeout(Number(process.env.SHOT_WAIT || 300));
          for (const button of (process.env.SHOT_CLICK || '').split(',').filter(Boolean)) {
            await page.getByRole('button', { name: button, exact: true }).first().click();
            await page.waitForTimeout(300);
          }
          for (const key of (process.env.SHOT_KEYS || '').split(',').filter(Boolean)) {
            await page.keyboard.press(key);
            await page.waitForTimeout(300);
          }
          if (process.env.SHOT_TYPE) {
            await page.keyboard.type(process.env.SHOT_TYPE);
            await page.waitForTimeout(300);
          }
          const name = where === '/' ? 'home' : where.slice(1).replace(/\.html$/, '').replace(/\//g, '-');
          const file = path.join(out, `${label}-${name}-${theme}-${form}.png`);
          await page.screenshot({ path: file, fullPage: form !== 'desktop' || !!process.env.SHOT_FULL });
          console.log(file);
        }
        await page.close();
      }
    }
  } finally {
    await browser.close();
    await server.close();
  }
})().catch(error => { console.error(error); process.exit(1); });
