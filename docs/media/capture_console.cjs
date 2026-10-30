// The product film's console pictures, rendered from rust/web with every /api
// request answered here: no host is contacted.
//
// cd rust/web && npm ci && cd ../..
// node docs/media/capture_console.cjs [outdir]
//
// Needs Playwright (PLAYWRIGHT_MODULE=<path to playwright> when it is not
// installed globally) and Chromium (CHROMIUM_EXECUTABLE=<chrome.exe>).
// Writes console/{home-ready,home,library,add-games,game,pair,pair-pin,coop}.jpg
// in dark at twice the pixels, plus console/boxes.json, the regions the film's
// camera moves to, for render_launch_film.py.
//
// The host, devices and games are made up; the covers in covers/ are original
// artwork, served from the console's own origin as the host serves app covers.
// The pairing QR code is a pattern drawn here that encodes nothing.
const fs = require('node:fs');
const path = require('node:path');
const { createRequire } = require('node:module');
const { pathToFileURL } = require('node:url');

const repo = path.resolve(__dirname, '..', '..');
const web = path.join(repo, 'rust', 'web');
const out = path.resolve(process.argv[2] || path.join(repo, 'docs', 'media'));
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || 'playwright');
// Vite and the Svelte plugin come from the console's own node_modules.
const fromWeb = createRequire(path.join(web, 'package.json'));

const now = Math.floor(Date.now() / 1000);
// Deterministic jitter for the live graphs.
let seed = 7;
const jitter = () => ((seed = (seed * 16807) % 2147483647) / 2147483647 - 0.5);
// Two minutes of host processing: steady, with small deterministic jitter.
const history = Array.from({ length: 120 }, (_, i) => ({
  host_processing_mean_ms: 2.75 + Math.sin(i / 9) * 0.12 + jitter() * 0.3 + (i === 71 ? 0.8 : 0),
}));
// Where a new picture's time goes on the host, in the last five seconds.
const timing = (mean_ms) => ({ mean_ms, p95_ms: +(mean_ms * 1.3).toFixed(2), p99_ms: +(mean_ms * 1.5).toFixed(2) });
const stages = () => ({
  frames: 600, window_seconds: 5, capture: timing(0.2), hold: timing(0.05), claim: timing(0.05),
  convert: timing(0.15), encode: timing(2.3), queue: timing(0.05), packetize: timing(0.1), send_queue: timing(0.05),
  send: timing(0.15), present_to_sent: timing(3.1), moonlight_host: timing(2.8),
});
const streamOf = (uuid, device_name, extra = {}) => ({
  uuid, device_name, role: 'stream', state: 'RUNNING',
  width: 2560, height: 1440, fps: 120, video_format: 2, encoder: 'amf', hdr: true, vrr: false,
  encoder_bitrate_kbps: 60000, fec_percent: 20, pyrowave_minimum_kbps: null, pyrowave_recommended_kbps: null,
  audio_channels: 2, frames_sent: 1, bytes_sent: 1, idr_requests: 0, uptime_seconds: 2537, warnings: [],
  performance: {
    fps: 119.9, bitrate_mbps: 58, encode_mean_ms: 2.6, encode_p95_ms: 3.4, encode_p99_ms: 3.9,
    host_processing_mean_ms: 2.8, host_processing_p95_ms: 3.5, host_processing_p99_ms: 4.1, host_processing_max_ms: 5.2,
    frame_age_mean_ms: 4.6, frame_age_p95_ms: 5.8, present_to_send_mean_ms: 1, send_interval_p99_ms: 9,
    sample_frames: 240, history, stages: stages(),
  },
  ...extra,
});
const stream = streamOf('deck', 'Steam Deck');
const metadata = {
  status: true, platform: 'windows', version: fromWeb('./package.json').version, host_name: 'Living room PC',
  pc_address: '192.168.1.20', pc_addresses: ['192.168.1.20'], paired_devices: 3, enable_pairing: true, warnings: [],
  encoder_status: { state: 'ready', h264: true, hevc: true, av1: true, pyrowave: true },
  capture_status: { configured_backend: 'auto', virtual_display_configured: true, displays: [{}, {}], error: null },
  virtual_display: { capable: true, ready: true, reason: '', protocol: 'x' }, gpus: [], gpu_error: null,
  audio_sinks: [{ name: 'Speakers', description: '', default: true }], audio_error: null, audio_enabled: true,
  features: {}, credentials_exists: true,
};
const clients = [
  { uuid: 'deck', name: 'Steam Deck', enabled: true, connected: true, perm: 0x071f1f00 },
  { uuid: 'tv', name: 'Living room TV', enabled: true, connected: false, perm: 0x071f1f00 },
  { uuid: 'laptop', name: 'Laptop', enabled: true, connected: false, perm: 0x00000300 },
];
const pixel = { uniqueid: 'pixel', name: 'Pixel 9 Pro', age_seconds: 6 };

// A QR-like picture: three finder squares and seeded modules. It is not a code.
function fakeQr() {
  const n = 29;
  let s = 11;
  const bit = () => ((s = (s * 48271) % 2147483647) % 100) < 46;
  const cells = [];
  const corners = [[0, 0], [n - 7, 0], [0, n - 7]];
  for (let y = 0; y < n; y++) {
    for (let x = 0; x < n; x++) {
      // A finder square with a one-module quiet border around it.
      const corner = corners.find(([cx, cy]) => x >= cx - 1 && x <= cx + 7 && y >= cy - 1 && y <= cy + 7);
      if (!corner) {
        if (bit()) cells.push([x, y]);
        continue;
      }
      const ring = Math.max(Math.abs(x - corner[0] - 3), Math.abs(y - corner[1] - 3));
      if (ring <= 3 && ring !== 2) cells.push([x, y]);
    }
  }
  const rects = cells.map(([x, y]) => `<rect x="${x + 2}" y="${y + 2}" width="1" height="1"/>`).join('');
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${n + 4} ${n + 4}" shape-rendering="crispEdges">`
    + `<rect width="${n + 4}" height="${n + 4}" fill="#fff"/><g fill="#111">${rects}</g></svg>`;
}
const pairingCodes = { status: true, name: 'Living room PC', expires_in_seconds: 165,
  codes: [{ address: '192.168.1.20', svg: fakeQr(), uri: 'nightjar://pair?sample' }] };

// The made-up library: each game has a cover in covers/<uuid>.webp. The first
// four fill the Home shelf.
const covers = path.join(__dirname, 'covers');
const game = (uuid, name, extra = {}) => ({
  uuid, name, cmd: '', 'working-dir': '', 'prep-cmd': [], 'image-path': `covers/${uuid}.webp`, ...extra,
});
const library = [
  game('ashen-crown', 'Ashen Crown', {
    'radeon-profile': 'custom',
    'radeon-settings': { 'anti_lag.enabled': true, 'frame_rate_target.enabled': true, 'frame_rate_target.fps': 120,
      'chill.enabled': true, 'chill.min_fps': 60, 'chill.max_fps': 120 },
  }),
  game('redline-avenue', 'Redline Avenue'),
  game('starfall-vanguard', 'Starfall Vanguard'),
  game('pip-and-bloom', 'Pip & Bloom'),
  game('tidebound', 'Tidebound'),
  game('chrome-syndicate', 'Chrome Syndicate'),
  game('hollow-ward', 'Hollow Ward'),
  game('skyline-strikers', 'Skyline Strikers'),
  game('iron-meridian', 'Iron Meridian'),
  game('frostline', 'Frostline'),
  game('lumen-grove', 'Lumen Grove'),
  game('dust-runners', 'Dust Runners'),
  game('turbo-critters', 'Turbo Critters'),
  game('orbital-siege', 'Orbital Siege'),
];
const playing = 'Ashen Crown';
// Add games: one scan across every launcher the host reads.
const launchers = [
  ['steam', 'Steam'], ['epic', 'Epic Games'], ['battlenet', 'Battle.net'], ['ea', 'EA app'],
  ['ubisoft', 'Ubisoft Connect'], ['gog', 'GOG'], ['microsoft', 'Microsoft Store and Xbox'],
];
const found = (source, uuid, name, extra = {}) => ({
  source, id: `${source}-${uuid}`, uuid, name, install_dir: `D:\\Games\\${name}`, already_added: false,
  unavailable_reason: null, has_cover: true, ...extra,
});
const discovery = {
  status: true,
  sources: launchers.map(([source, name]) => ({ source, name, state: 'available', errors: [] })),
  games: [
    found('steam', 'ashen-crown', 'Ashen Crown', { already_added: true }),
    found('epic', 'redline-avenue', 'Redline Avenue'),
    found('battlenet', 'orbital-siege', 'Orbital Siege'),
    found('ea', 'skyline-strikers', 'Skyline Strikers'),
    found('ubisoft', 'tidebound', 'Tidebound'),
    found('gog', 'lumen-grove', 'Lumen Grove'),
    found('microsoft', 'frostline', 'Frostline'),
    found('steam', 'dust-runners', 'Dust Runners'),
    found('epic', 'turbo-critters', 'Turbo Critters'),
    found('gog', 'hollow-ward', 'Hollow Ward'),
  ],
};

// What the driver offers, as an RX 7900 XT reports it, so the Radeon controls show.
const toggle = (id, group, value = false) => ({ id, group, kind: 'toggle', state: 'configurable', value });
const integer = (id, group, value, min, max) => ({ id, group, kind: 'integer', state: 'configurable', value,
  range: { min, max, step: 1 }, unit: 'fps' });
const radeon = { status: true, state: 'configurable', gpus: [{ id: 0, name: 'AMD Radeon GPU', driver: '', settings: [
  toggle('anti_lag.enabled', 'Anti-Lag'),
  toggle('frame_rate_target.enabled', 'Frame Rate Target Control'), integer('frame_rate_target.fps', 'Frame Rate Target Control', 120, 30, 300),
  toggle('chill.enabled', 'Chill'), integer('chill.min_fps', 'Chill', 60, 30, 300), integer('chill.max_fps', 'Chill', 120, 30, 300),
  toggle('image_sharpening.enabled', 'Image Sharpening'),
  toggle('anisotropic_filtering.enabled', 'Anisotropic Filtering'),
  toggle('boost.enabled', 'Boost'),
] }] };

// Co-op: three devices in one game, each with its own instance.
const coopGame = 'Turbo Critters';
const coop = {
  mode: { setting: 'nucleus', mode: 'nucleus', label: 'Nucleus Co-op', reason: '' },
  install: { state: 'installed', path: 'C:\\Program Files\\Rubylight\\nucleus', version: '2.3', error: null },
  available: true, reason: null, elevates: true, opened: false,
  games: [{ app: 'turbo-critters', name: coopGame, status: 'ready', reason: null, handler: coopGame, players: 4,
    runs: 'user', entry: `${coopGame} (Co-op)` }],
  sessions: [{ id: 1, kind: 'coop', app_id: 13, title: coopGame, game: coopGame, phase: 'running', end: null, kicked: [],
    players: [
      { slot: 0, device: 'tv', name: 'Living room TV', away: false },
      { slot: 1, device: 'deck', name: 'Steam Deck', away: false },
      { slot: 2, device: 'laptop', name: 'Laptop', away: false },
      { slot: 3, device: null, name: null, away: false },
    ] }],
};
const coopStreams = [streamOf('tv', 'Living room TV', { width: 3840, height: 2160, fps: 60 }),
  streamOf('deck', 'Steam Deck', { width: 1280, height: 800, fps: 90, hdr: false }),
  streamOf('laptop', 'Laptop', { width: 1920, height: 1080, fps: 60, hdr: false })];

const pairing = { path: '/devices', pending: [pixel], apps: library,
  boxes: { panel: 'section.panel:has(.ways)', qr: '.way:has(#pair-qr)', ways: '.ways',
    request: 'form.request', pin: 'form.request input.pin' } };
const openPanel = (title) => async (page) => {
  await page.locator(`details:has(> summary .title:text-is("${title}")) > summary`).click();
};

// Each picture: the console page, the mocked state, and the regions to record.
const scenes = {
  'home-ready': { path: '/', apps: library, fit: true, boxes: { hero: 'section.hero' } },
  home: {
    path: '/', streams: [stream], app: playing, apps: library, fit: true,
    act: async (page) => { await page.locator('summary:text-is("Stream details & performance")').click(); },
    boxes: { hero: 'section.hero', card: 'article.stream', details: 'article.stream details.performance',
      shelf: 'section.games' },
  },
  library: { path: '/library', app: playing, apps: library, boxes: { grid: 'ul.grid' } },
  'add-games': {
    path: '/library/import', apps: library, height: 1240,
    act: async (page) => {
      for (const name of ['Redline Avenue', 'Skyline Strikers', 'Tidebound']) {
        await page.getByRole('checkbox', { name: new RegExp(`^Add ${name} from`) }).check();
      }
      await page.evaluate(() => window.scrollTo(0, 0));
    },
    boxes: { panel: 'section.panel:has(ul.games)', games: 'ul.games' },
  },
  game: {
    path: '/library/ashen-crown', app: playing, apps: library, fit: true,
    act: async (page) => {
      await openPanel('Display')(page);
      await openPanel('Radeon')(page);
    },
    boxes: {
      display: 'section.panel:has(> details > summary .title:text-is("Display"))',
      video: 'section.panel:has(> details > summary .title:text-is("Video and input"))',
      radeon: 'section.panel:has(> details > summary .title:text-is("Radeon"))',
      device: 'section.panel:has(> details > summary .title:text-is("Per device"))',
    },
  },
  // Pairing, before and after the PIN is typed.
  pair: { ...pairing },
  'pair-pin': {
    ...pairing,
    act: async (page) => { await page.getByLabel(`PIN shown on ${pixel.name}`).fill('4821'); },
  },
  coop: {
    path: '/', streams: coopStreams, app: `${coopGame} (Co-op)`, apps: library, coop, fit: true,
    boxes: { card: `section.panel:has(:text-is("${coopGame}"))` },
  },
};

function answer(scene) {
  return (url) => {
    const p = url.pathname;
    const streams = scene.streams || [];
    if (p === '/api/auth/status') return { status: true, credentials_configured: true, authenticated: true };
    if (p === '/api/csrf-token') return { csrf_token: 'film' };
    if (p === '/api/metadata') return metadata;
    if (p === '/api/clients/pending') return { status: true, requests: scene.pending || [] };
    if (p === '/api/clients/list') return { status: true, clients };
    if (p === '/api/clients/hdr-profiles') return { status: true, profiles: [] };
    if (p === '/api/display-devices') return [];
    if (p === '/api/rtsp/sessions') return { status: true, sessions: streams };
    if (p === '/api/session/status') {
      return { status: true, activeSessions: streams.length, appRunning: !!scene.app, appName: scene.app || '',
        paused: false, running: !!scene.app, app: scene.app ? { name: scene.app } : null };
    }
    if (p === '/api/apps') return { status: true, apps: scene.apps || [] };
    if (p === '/api/library/discovery') return discovery;
    if (p === '/api/radeon') return radeon;
    if (p === '/api/usb') return { status: true, devices: [] };
    if (p === '/api/coop') return scene.coop || { ...coop, sessions: [] };
    if (p === '/api/steam/status' || p === '/api/playnite/status') return { status: true, enabled: false, available: false };
    if (p === '/api/logs/tail') return { offset: 0, text: '', reset: false };
    if (p === '/api/updates') {
      return { status: true, checking: false, check_failed: false, checked_at: now - 600, releases: [],
        update_available: false, latest_version: '', install_supported: true, auto_update: false,
        update_check_interval: 6, phase: 'idle' };
    }
    if (p === '/api/auth/sessions') return { status: true, sessions: [] };
    if (p === '/api/tokens') return { status: true, tokens: [] };
    if (p === '/api/token/routes') return { status: true, routes: [] };
    if (p === '/api/config') return { sunshine_name: 'Living room PC' };
    if (p === '/api/session/console') {
      return { status: true, recovery: { session_id: 1, console_session_id: 1, can_reattach: false, reason: '' } };
    }
    return { status: true, available: false, enabled: false };
  };
}

// The console shows the build version in two places; leave it out so the
// pictures stay current across releases.
const HIDE_VERSION = `
  aside .host > .muted { display: none !important; }
  .facts dt:nth-last-of-type(1), .facts dd:nth-last-of-type(1) { display: none !important; }
`;

async function shoot(browser, base, name, { width, height: frame, scale, file }) {
  const scene = scenes[name];
  const height = scene.height || frame;
  const context = await browser.newContext({ viewport: { width, height }, deviceScaleFactor: scale, colorScheme: 'dark' });
  await context.addInitScript(() => {
    try { localStorage.setItem('butterpollo.theme', 'dark'); } catch { /* the console falls back to system */ }
  });
  const page = await context.newPage();
  // Vite compiles a page's modules on first use, which can take a while.
  page.setDefaultTimeout(30000);
  const errors = [];
  page.on('pageerror', (error) => errors.push(error.message));
  const reply = answer(scene);
  await page.route('**/*', async (route) => {
    const url = new URL(route.request().url());
    if (url.origin !== base) {
      errors.push(`blocked a request outside the console: ${url.origin}`);
      return route.abort();
    }
    if (!url.pathname.startsWith('/api/')) return route.continue();
    if (route.request().method() !== 'GET') {
      if (url.pathname === '/api/pairing/qr') return route.fulfill({ json: pairingCodes });
      return route.fulfill({ json: { status: true } });
    }
    const cover = /^\/api\/apps\/([a-z0-9-]+)\/cover$/.exec(url.pathname);
    if (cover) {
      const file = path.join(covers, `${cover[1]}.webp`);
      if (!fs.existsSync(file)) return route.fulfill({ status: 404, body: '' });
      return route.fulfill({ contentType: 'image/webp', body: fs.readFileSync(file) });
    }
    return route.fulfill({ json: reply(url) });
  });
  await page.goto(base + scene.path);
  await page.waitForLoadState('networkidle');
  await page.addStyleTag({ content: HIDE_VERSION });
  if (scene.act) await scene.act(page);
  // Park the pointer on the sidebar so no field shows a hover ring.
  await page.mouse.move(120, 700);
  if (scene.fit) {
    // Grow the window to the whole page, so the sidebar runs its full height too.
    for (let pass = 0; pass < 2; pass++) {
      const tall = await page.evaluate(() => document.documentElement.scrollHeight);
      await page.setViewportSize({ width, height: Math.max(height, tall) });
    }
    await page.evaluate(() => window.scrollTo(0, 0));
  }
  // Settle polling, fonts and transitions.
  await page.waitForTimeout(900);
  // The film's sources are photos of a page more than diagrams: a fine JPEG keeps
  // them sharp at a tenth of the PNG's size.
  await page.screenshot({ path: path.join(out, file), type: 'jpeg', quality: 92 });
  const boxes = {};
  for (const [key, selector] of Object.entries(scene.boxes || {})) {
    const box = await page.locator(selector).first().boundingBox();
    if (!box) throw new Error(`${name}: ${selector} is not on the page`);
    boxes[key] = [box.x, box.y, box.x + box.width, box.y + box.height].map((v) => Math.round(v * scale));
  }
  await context.close();
  if (errors.length) throw new Error(`${name}: ${errors.join('; ')}`);
  console.log(`Wrote ${path.join(out, file)}`);
  return boxes;
}

(async () => {
  fs.mkdirSync(path.join(out, 'console'), { recursive: true });
  const { createServer } = await import(pathToFileURL(fromWeb.resolve('vite')).href);
  const { svelte } = await import(pathToFileURL(fromWeb.resolve('@sveltejs/vite-plugin-svelte')).href);
  // configFile: false leaves out the dev proxy, so nothing can reach a host.
  const server = await createServer({
    root: web, configFile: false, plugins: [svelte()], logLevel: 'error',
    server: { host: '127.0.0.1', port: 0, hmr: false },
  });
  await server.listen();
  const base = `http://127.0.0.1:${server.httpServer.address().port}`;
  const browser = await chromium.launch({ headless: true,
    ...(process.env.CHROMIUM_EXECUTABLE ? { executablePath: process.env.CHROMIUM_EXECUTABLE } : {}),
  });
  try {
    // Twice the pixels, so a zoom stays sharp at 1080p.
    const film = { width: 1440, height: 900, scale: 2 };
    const boxes = { scale: film.scale };
    for (const name of Object.keys(scenes)) {
      boxes[name] = await shoot(browser, base, name, { ...film, file: path.join('console', `${name}.jpg`) });
    }
    // Screenshot pixels, [left, top, right, bottom], one region per line.
    const json = JSON.stringify(boxes, null, 2).replace(/\[\s+([\d,\s]+?)\s+\]/g, (_, v) => `[${v.split(/,\s*/).join(', ')}]`);
    fs.writeFileSync(path.join(out, 'console', 'boxes.json'), `${json}\n`);
  } finally {
    await browser.close();
    await server.close();
  }
})().catch((error) => {
  console.error(error);
  process.exit(1);
});
