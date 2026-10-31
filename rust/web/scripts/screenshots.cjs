// Console screenshots against fixture data, for design review. Never contacts a host.
//   PLAYWRIGHT_MODULE=<path to playwright> node scripts/screenshots.cjs <out-dir> [label]
// Writes <label>-<page>-<theme>-<form>.png for each page, theme and form factor.
// SHOT_PAGES=/,/devices picks pages (/design.html is the component gallery);
// SHOT_FULL=1 captures the full page on desktop too (phones always are).
// SHOT_CLICK=Select,"Select all" clicks those buttons, in order, before each shot.
// SHOT_AUTH=signed-out shows the sign-in page, SHOT_AUTH=setup first-run setup
// (the fixture is signed in otherwise); either one shoots that page by default.
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || 'playwright');
const path = require('node:path');
const fs = require('node:fs');

const out = path.resolve(process.argv[2] || 'shots');
const label = process.argv[3] || 'shot';
const auth = process.env.SHOT_AUTH || '';
if (auth && !['signed-out', 'setup'].includes(auth)) throw new Error(`SHOT_AUTH is signed-out or setup, not ${auth}`);
const pages = (process.env.SHOT_PAGES || { 'signed-out': '/login', setup: '/setup' }[auth] || '/,/devices,/settings,/maintenance').split(',');
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
  '/api/config': { sunshine_name: 'Gaming PC', enable_pairing: false },
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
  // Nucleus Co-op as the host installs it (rust/core/src/nucleus/install.rs VERSION and SIZE).
  '/api/coop': { mode: { setting: 'nucleus', mode: 'nucleus', label: 'Nucleus Co-op', reason: '' },
    install: { state: 'installed', path: 'C:\\ProgramData\\Butterpollo\\nucleus', version: '2.4.2', error: null,
      release: { version: '2.4.2', size: 97_396_862 }, received: null },
    available: true, reason: null, elevates: true, admin_protection: false, open_refusal: null, opened: false,
    games: [], catalog_error: null, sessions: [] },
  '/api/logs/analysis': { status: true, findings: [], since: '2026-10-31T08:02:11Z', until: '2026-10-31T09:41:57Z', read_bytes: 812_544 },
  '/api/session/console': { status: true, recovery: { session_id: 1, console_session_id: 1, can_reattach: false,
    reason: 'This host is already attached to the physical console.' } },
  '/api/display/golden_status': { status: true, exists: true, comparison_available: true, current_mismatch_reason: '' },
  '/api/frame-limiter/status': { enabled: false, configured_provider: 'none', active_provider: 'none', resolved_path: '' },
  '/api/power': {
    status: true,
    wake: { revision: 1, primary: '9c:6b:00:12:34:56', targets: [
      { mac: '9c:6b:00:12:34:56', kind: 'ethernet', connected: true, route: true, broadcasts: ['192.168.4.255'], state: 'needs_setup',
        checks: [{ id: 'wake_enabled', state: 'error' }, { id: 'magic_packet', state: 'ok' }, { id: 'magic_packet_only', state: 'warning' }] },
      { mac: '9c:6b:00:ab:cd:ef', kind: 'wifi', connected: false, route: false, broadcasts: [], state: 'experimental', checks: [] },
    ], system: { sleep: 's3', modern_standby: false, hibernate: true, fast_startup: 'active', shutdown_wake: 'firmware_unknown' } },
    idle_sleep: { state: 'armed', minutes: 30, reason: 'waiting for console activity', remaining_seconds: 1260 },
    last_sleep: null,
    fix: { running: false, changes: ['Realtek PCIe 2.5GbE: allow this device to wake the computer',
      'Realtek PCIe 2.5GbE: only allow a magic packet to wake the computer'], skipped: [], error: null,
    fast_startup_off_offered: true, last: null },
  },
};

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
          if (route.request().method() !== 'GET') return route.fulfill({ json: { status: true } });
          return route.fulfill({ json: replies[url.pathname] ?? { status: true, available: false, enabled: false } });
        });
        for (const where of pages) {
          await page.goto(base + where);
          await page.waitForLoadState('networkidle');
          await page.waitForTimeout(300);
          for (const button of (process.env.SHOT_CLICK || '').split(',').filter(Boolean)) {
            await page.getByRole('button', { name: button, exact: true }).first().click();
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
