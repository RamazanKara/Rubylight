// Screenshots of the web console for the demo film and the README, rendered
// from rust/web with every /api request answered here: no host is contacted.
//
// cd rust/web && npm ci && cd ../..
// node docs/media/capture_console.cjs [outdir] [readme|film]
//
// Needs Playwright (PLAYWRIGHT_MODULE=<path to playwright> when it is not
// installed globally) and Chromium (CHROMIUM_EXECUTABLE=<chrome.exe>).
// Writes the README pictures, console-{dark,light}.png (Home) and
// console-{library,add-games,radeon}-{dark,light}.png, and
// console/{overview,devices,settings}.png plus console/boxes.json, the
// regions the film's camera moves to, for render_launch_film.py.
//
// The games are made up and their covers in covers/ are original artwork,
// served from the console's own origin as the host serves app covers.
const fs = require('node:fs');
const path = require('node:path');
const { spawnSync } = require('node:child_process');
const { createRequire } = require('node:module');
const { pathToFileURL } = require('node:url');

const repo = path.resolve(__dirname, '..', '..');
const web = path.join(repo, 'rust', 'web');
const out = path.resolve(process.argv[2] || path.join(repo, 'docs', 'media'));
// readme, film, or both when left out.
const only = process.argv[3] || '';
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || 'playwright');
// Vite and the Svelte plugin come from the console's own node_modules.
const fromWeb = createRequire(path.join(web, 'package.json'));

const now = Math.floor(Date.now() / 1000);
// Two minutes of host processing: steady, with small deterministic jitter.
let seed = 7;
const jitter = () => ((seed = (seed * 16807) % 2147483647) / 2147483647 - 0.5);
const history = Array.from({ length: 120 }, (_, i) => ({
  host_processing_mean_ms: 2.75 + Math.sin(i / 9) * 0.12 + jitter() * 0.3 + (i === 71 ? 0.8 : 0),
}));
const stream = {
  uuid: 'deck', device_name: 'Steam Deck', role: 'stream', state: 'RUNNING',
  width: 2560, height: 1440, fps: 120, video_format: 2, encoder: 'amf', hdr: true, vrr: false,
  encoder_bitrate_kbps: 60000, fec_percent: 20, pyrowave_minimum_kbps: null, pyrowave_recommended_kbps: null,
  audio_channels: 2, frames_sent: 1, bytes_sent: 1, idr_requests: 0, uptime_seconds: 2537, warnings: [],
  performance: {
    fps: 119.9, bitrate_mbps: 58, encode_mean_ms: 2.6, encode_p95_ms: 3.4, encode_p99_ms: 3.9,
    host_processing_mean_ms: 2.8, host_processing_p95_ms: 3.5, host_processing_p99_ms: 4.1, host_processing_max_ms: 5.2,
    frame_age_mean_ms: 4.6, frame_age_p95_ms: 5.8, present_to_send_mean_ms: 1, send_interval_p99_ms: 9,
    sample_frames: 240, history,
  },
};
const metadata = {
  status: true, platform: 'windows', version: fromWeb('./package.json').version, host_name: 'Living room PC',
  pc_address: '192.168.1.20', pc_addresses: ['192.168.1.20'], paired_devices: 3, enable_pairing: true, warnings: [],
  encoder_status: { state: 'ready', h264: true, hevc: true, av1: true, pyrowave: true },
  capture_status: { configured_backend: 'auto', virtual_display_configured: true, displays: [{}, {}], error: null },
  virtual_display: { capable: true, ready: true, reason: '', protocol: 'x' }, gpus: [], gpu_error: null,
  audio_sinks: [{ name: 'Speakers (Realtek)', description: '', default: true }], audio_error: null, audio_enabled: true,
  features: {}, credentials_exists: true,
};
const clients = [
  { uuid: 'deck', name: 'Steam Deck', enabled: true, connected: true, perm: 0x071f1f00 },
  { uuid: 'tv', name: 'Living room TV', enabled: true, connected: false, perm: 0x071f1f00 },
  { uuid: 'laptop', name: 'Laptop', enabled: true, connected: false, perm: 0x00000300 },
];
const pixel = { uniqueid: 'pixel', name: 'Pixel 9 Pro', age_seconds: 6 };

// The library of the README pictures: made-up games, each with a cover in
// covers/<uuid>.webp. The first four fill the Home shelf.
const covers = path.join(__dirname, 'covers');
const game = (uuid, name, extra = {}) => ({
  uuid, name, cmd: '', 'working-dir': '', 'prep-cmd': [], 'image-path': `covers/${uuid}.webp`, ...extra,
});
const library = [
  game('ashen-crown', 'Ashen Crown', {
    'radeon-profile': 'custom',
    'radeon-settings': { 'anti_lag.enabled': true, 'frame_rate_target.enabled': true, 'frame_rate_target.fps': 120,
      'anisotropic_filtering.enabled': true },
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

// Each scene: the console page, the mocked state, and the regions to record.
// The film's scenes keep the plain two-app library.
const scenes = {
  overview: {
    film: true, path: '/', streams: [stream], app: 'Steam Big Picture',
    boxes: { card: 'article.stream', spec: 'article.stream ul.spec', metrics: 'article.stream dl.metrics',
      readiness: 'section.panel:has(:text-is("Host readiness"))' },
  },
  devices: {
    film: true, path: '/devices', pending: [pixel],
    act: (page) => page.getByLabel(`PIN shown on ${pixel.name}`).fill('4821'),
    boxes: { pair: 'section.panel:has([role=tablist])', tabs: '[role=tablist]', request: 'form.request',
      paired: 'section.panel:has(:text-is("Paired devices"))' },
  },
  settings: {
    film: true, path: '/settings/video',
    boxes: { capture: 'section.group:has(h3:text-is("Capture"))', codecs: 'section.group:has(h3:text-is("Codecs"))',
      categories: 'nav[aria-label="Settings categories"]' },
  },
  // README pictures.
  home: { path: '/', streams: [stream], app: playing, apps: library, fit: true },
  library: { path: '/library', app: playing, apps: library },
  'add-games': {
    // Tall enough for a game from each of the first six launchers.
    path: '/library/import', apps: library, height: 1240,
    act: async (page) => {
      for (const name of ['Redline Avenue', 'Skyline Strikers', 'Tidebound']) {
        await page.getByRole('checkbox', { name: new RegExp(`^Add ${name} from`) }).check();
      }
      await page.evaluate(() => window.scrollTo(0, 0));
    },
  },
  radeon: {
    path: '/library/ashen-crown', app: playing, apps: library,
    act: async (page) => {
      await page.locator('details:has(> summary .title:text-is("Radeon")) > summary').click();
      // From the Display panel down, so the open Radeon panel sits between its neighbours.
      await page.locator('section.panel:has(> details > summary .title:text-is("Display"))')
        .evaluate((panel) => window.scrollTo(0, panel.getBoundingClientRect().top + window.scrollY - 24));
      // Park the pointer on the sidebar so no field shows a hover ring.
      await page.mouse.move(120, 500);
    },
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
    if (p === '/api/apps') {
      return { status: true, apps: scene.apps || [{ uuid: 'a1', name: 'Desktop' }, { uuid: 'a2', name: 'Steam Big Picture' }] };
    }
    if (p === '/api/library/discovery') return discovery;
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

async function shoot(browser, base, name, scheme, { width, height: frame, scale, file }) {
  const scene = scenes[name];
  const height = scene.height || frame;
  const context = await browser.newContext({ viewport: { width, height }, deviceScaleFactor: scale, colorScheme: scheme });
  await context.addInitScript((theme) => {
    try { localStorage.setItem('butterpollo.theme', theme); } catch { /* the console falls back to system */ }
  }, scheme);
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
    if (route.request().method() !== 'GET') return route.fulfill({ json: { status: true } });
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
  if (scene.fit) {
    // Grow the window to the whole page, so the sidebar runs its full height too.
    for (let pass = 0; pass < 2; pass++) {
      const tall = await page.evaluate(() => document.documentElement.scrollHeight);
      await page.setViewportSize({ width, height: Math.max(height, tall) });
    }
    await page.evaluate(() => window.scrollTo(0, 0));
  }
  // Settle polling, fonts and transitions.
  await page.waitForTimeout(800);
  await page.screenshot({ path: path.join(out, file) });
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

// Cover art makes full-colour PNGs over a megabyte; a dithered 256-colour
// palette (ffmpeg on PATH) keeps a README picture under about 500 KB.
function shrink(file) {
  const small = `${file}.palette.png`;
  const run = spawnSync('ffmpeg', ['-loglevel', 'error', '-y', '-i', file, '-lavfi',
    'split[a][b];[a]palettegen=max_colors=256:stats_mode=full[p];[b][p]paletteuse=dither=sierra2_4a', small]);
  if (run.error || run.status !== 0) {
    console.warn(`ffmpeg could not shrink ${file}; kept it full colour`);
    fs.rmSync(small, { force: true });
    return;
  }
  if (fs.statSync(small).size < fs.statSync(file).size) fs.renameSync(small, file);
  else fs.rmSync(small);
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
    if (only !== 'film') {
      // README: 1440 pixels wide, for a <picture> with both colour schemes.
      const readme = { home: 'console', library: 'console-library', 'add-games': 'console-add-games', radeon: 'console-radeon' };
      for (const [name, file] of Object.entries(readme)) {
        for (const scheme of ['dark', 'light']) {
          await shoot(browser, base, name, scheme, { width: 1440, height: 1000, scale: 1, file: `${file}-${scheme}.png` });
          shrink(path.join(out, `${file}-${scheme}.png`));
        }
      }
    }
    if (only !== 'readme') {
      // Film sources: twice the pixels, so a zoom stays sharp at 1080p.
      const film = { width: 1440, height: 900, scale: 2 };
      const boxes = { scale: film.scale };
      for (const name of Object.keys(scenes).filter((key) => scenes[key].film)) {
        boxes[name] = await shoot(browser, base, name, 'dark', { ...film, file: path.join('console', `${name}.png`) });
      }
      // Screenshot pixels, [left, top, right, bottom], one region per line.
      const json = JSON.stringify(boxes, null, 2).replace(/\[\s+([\d,\s]+?)\s+\]/g, (_, v) => `[${v.split(/,\s*/).join(', ')}]`);
      fs.writeFileSync(path.join(out, 'console', 'boxes.json'), `${json}\n`);
    }
  } finally {
    await browser.close();
    await server.close();
  }
})().catch((error) => {
  console.error(error);
  process.exit(1);
});
