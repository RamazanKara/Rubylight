// The README's console pictures and its console tour, rendered from rust/web with
// every /api request answered here: no host is contacted.
//
// cd rust/web && npm ci && cd ../..
// node docs/media/capture_readme.cjs <outdir> [stills|tour]
//
// Needs Playwright and Chromium, as capture_console.cjs does (PLAYWRIGHT_MODULE,
// CHROMIUM_EXECUTABLE), and takes the made-up host, devices, games and covers from
// it. Writes <outdir>/stills/<page>-<theme>.png, light and dark at twice the pixels,
// and <outdir>/tour/frame-*.jpg with frames.txt, the list FFmpeg's concat demuxer
// reads. build_readme_media.py turns both into the files the README shows.
//
// No picture shows an address, a MAC, a PIN or a pairing code, and the build version
// is left out so the pictures stay current across releases.
const fs = require('node:fs');
const path = require('node:path');
const film = require('./capture_console.cjs');

const out = path.resolve(process.argv[2] || 'readme-media');
const what = process.argv[3] || 'all';
const now = Math.floor(Date.now() / 1000);

// The library with the launcher each game came from, so Games shows its store filters.
const sources = ['steam', 'epic', 'steam', 'gog', 'ubisoft', 'steam', 'battlenet', 'ea', 'steam', 'microsoft', 'gog', 'steam', 'epic', 'battlenet'];
const library = film.library.map((game, i) => ({
  ...game, 'launcher-source': sources[i], 'launcher-id': game.uuid, 'synced-by': 'launcher',
  'last-played': now - (i + 1) * 43_200,
}));

// Streams that ended, newest first.
const ended = (ago, ran, device, name, app, extra) => ({
  id: `stream-${ago}`, device, device_name: name, app_id: 1, app, role: 'stream', transport: 'moonlight', codec: 'hevc',
  width: 1920, height: 1080, fps: 120, hdr: false, bitrate_kbps: 50_000, requested_bitrate_kbps: 50_000, mean_bitrate_kbps: 48_400,
  mean_fps: 119.6, p95_fps: 118.2, loss_percent: 0.02, end_reason: 'ended', end_detail: '', end_category: null,
  started_unix_ms: (now - ago - ran) * 1000, ended_unix_ms: (now - ago) * 1000, duration_ms: ran * 1000, ...extra,
});
const sessionHistory = [
  ended(900, 5_400, 'deck', 'Steam Deck', 'Ashen Crown', { width: 1280, height: 800, fps: 90, mean_fps: 89.8, p95_fps: 88.9,
    bitrate_kbps: 30_000, requested_bitrate_kbps: 30_000, mean_bitrate_kbps: 28_900 }),
  ended(22_000, 7_800, 'tv', 'Living room TV', 'Starfall Vanguard', { codec: 'av1', width: 3840, height: 2160, fps: 60, hdr: true,
    mean_fps: 59.9, p95_fps: 59.2, bitrate_kbps: 80_000, requested_bitrate_kbps: 80_000, mean_bitrate_kbps: 77_300, loss_percent: 0 }),
  ended(95_000, 2_700, 'laptop', 'Laptop', 'Redline Avenue', { codec: 'pyrowave', width: 2560, height: 1440, fps: 120,
    bitrate_kbps: 400_000, requested_bitrate_kbps: 400_000, mean_bitrate_kbps: 391_000, loss_percent: 0 }),
  ended(180_000, 4_100, 'deck', 'Steam Deck', 'Pip & Bloom', { width: 1280, height: 800, fps: 90, mean_fps: 89.7, p95_fps: 88.4,
    bitrate_kbps: 30_000, requested_bitrate_kbps: 30_000, mean_bitrate_kbps: 27_800, loss_percent: 0.11 }),
  ended(270_000, 9_300, 'tv', 'Living room TV', 'Tidebound', { codec: 'av1', width: 3840, height: 2160, fps: 60, hdr: true,
    mean_fps: 59.8, p95_fps: 58.9, bitrate_kbps: 80_000, requested_bitrate_kbps: 80_000, mean_bitrate_kbps: 76_100, loss_percent: 0.03 }),
  ended(360_000, 3_300, 'laptop', 'Laptop', 'Chrome Syndicate', { width: 2560, height: 1440, fps: 120, hdr: true,
    bitrate_kbps: 60_000, requested_bitrate_kbps: 60_000, mean_bitrate_kbps: 57_200, loss_percent: 0.05 }),
];

// The running stream, with the stutter report the stream card reads.
const stream = {
  ...film.streamOf('deck', 'Steam Deck'),
  stutter: { windows: 507, pictures: 304_200, late_pictures: 212, lost_ms: 498, dominant: 'encode',
    stages: [{ id: 'encode', lost_ms: 334, percent: 67 }, { id: 'send', lost_ms: 110, percent: 22 }, { id: 'capture', lost_ms: 54, percent: 11 }],
    not_measured: ['game', 'network', 'decode', 'present'] },
};

// Wake-on-LAN set up, as Settings reports it, with no adapter address to show.
const power = {
  status: true,
  wake: { revision: 1, primary: '', targets: [
    { mac: '', kind: 'ethernet', connected: true, route: true, broadcasts: [], state: 'ready',
      checks: [{ id: 'wake_enabled', state: 'ok' }, { id: 'magic_packet', state: 'ok' }, { id: 'magic_packet_only', state: 'ok' }] },
  ], system: { sleep: 's3', modern_standby: false, hibernate: true, fast_startup: 'active', shutdown_wake: 'firmware_unknown' } },
  idle_sleep: { state: 'off', minutes: 0, reason: '', remaining_seconds: 0 },
  last_sleep: null,
  fix: { running: false, changes: [], skipped: [], error: null, fast_startup_off_offered: false, last: null },
};

// Stream DVR: the replay buffer of the running stream and the clips saved from earlier ones, newest first.
const GB = 1024 ** 3;
const clip = (ago, app_title, seconds, kind, extra = {}) => ({
  id: `clip-${String(ago).padStart(8, '0')}`, app_title, created_unix_ms: (now - ago) * 1000, duration_ms: seconds * 1000,
  width: 2560, height: 1440, fps: 120, codec: 'hevc', hdr: true, container: 'mp4', size_bytes: Math.round(seconds * 7_300_000),
  kind, recovered: false, ...extra,
});
const clips = [
  clip(420, 'Ashen Crown', 60, 'replay'),
  clip(2_700, 'Ashen Crown', 30, 'replay'),
  clip(23_400, 'Starfall Vanguard', 312, 'manual', { width: 3840, height: 2160, fps: 60, codec: 'av1', size_bytes: 1_610_000_000 }),
  clip(97_000, 'Redline Avenue', 90, 'replay', { hdr: false, codec: 'h264' }),
  clip(183_000, 'Pip & Bloom', 45, 'replay', { width: 1280, height: 800, fps: 90, hdr: false }),
];
const clipBytes = clips.reduce((sum, c) => sum + c.size_bytes, 0);
const dvr = (streaming) => ({
  enabled: true, state: streaming ? 'armed' : 'off', reason: streaming ? 'none' : 'not_streaming',
  ring_buffered_secs: streaming ? 120 : 0, ring_capacity_secs: 120, recording: false, recording_elapsed_secs: 0,
  quota_used_bytes: clipBytes, quota_limit_bytes: 20 * GB, clips_dir_available: true, container: 'mp4',
  clips_dir: 'D:\\Clips',
});

// Displays as Windows arranges them: a monitor on the desk and the virtual display a stream uses.
const displays = [
  { device_id: 'desk', display_name: 'DISPLAY1', friendly_name: 'Desk monitor', hdr_supported: true, hdr_enabled: true, primary: true,
    x: 0, y: 0, width: 2560, height: 1440, refresh: 165, scale: 100, virtual: false, notes: [] },
  { device_id: 'side', display_name: 'DISPLAY2', friendly_name: 'Side monitor', hdr_supported: false, hdr_enabled: false, primary: false,
    x: -1080, y: -240, width: 1080, height: 1920, refresh: 60, scale: 100, virtual: false, notes: [] },
  { device_id: 'virtual', display_name: 'DISPLAY3', friendly_name: 'Rubylight virtual display', hdr_supported: true, hdr_enabled: true,
    primary: false, x: 2560, y: 0, width: 1280, height: 800, refresh: 90, scale: 100, virtual: true, notes: [] },
];

/** `film.answer` plus the replies the README's pages read. `state` is read on every request, so a tour can change it. */
function answer(state) {
  return (url) => {
    const scene = state.scene;
    const streaming = !!(scene.streams && scene.streams.length);
    if (url.pathname === '/api/dvr') return dvr(streaming);
    if (url.pathname === '/api/dvr/clips') {
      return { clips, next_cursor: null, quota_used_bytes: clipBytes, quota_limit_bytes: 20 * GB };
    }
    if (url.pathname === '/api/session/suspend') {
      return streaming
        ? { status: true, state: 'ready', reason: 'none', message: null, app_title: scene.app, suspended_count: 0 }
        : { status: true, state: 'unavailable', reason: 'no_game', message: null, app_title: null, suspended_count: 0 };
    }
    // Away wake relay on, with one always-on computer that checked in a few minutes ago.
    if (url.pathname === '/api/wake-relay') {
      return { enabled: true, epoch: 1, relays: [{ id: 'a'.repeat(64), name: 'Mini PC', enrolled_unix: now - 9 * 86_400, last_seen_unix: now - 160 }] };
    }
    if (url.pathname === '/api/display-devices') return displays;
    if (url.pathname === '/api/sessions/history') return { status: true, limit: 200, days: 90, bytes: 262_144, sessions: sessionHistory };
    if (url.pathname === '/api/logs/analysis') {
      return { status: true, findings: [], since: new Date((now - 5_400) * 1000).toISOString(), until: new Date(now * 1000).toISOString(), read_bytes: 812_544 };
    }
    if (url.pathname === '/api/neural') {
      return { accepted: true, available: true, gpu: 'AMD Radeon GPU', reason: null, found: true, enabled: false, exe: null,
        release: null, mode: null, game: null, earlier_upscaling: null, job: null };
    }
    if (url.pathname === '/api/metadata') {
      return { ...film.metadata, pc_address: '', pc_addresses: [],
        audio_sinks: [{ id: 'speakers', name: 'Speakers', description: '', adapter: '', default: true, virtual_sink: false }] };
    }
    if (url.pathname === '/api/power') return power;
    const reply = film.answer(scene)(url);
    // README_ROUTES=1 lists the routes no fixture answers, so a new console page shows what it still needs.
    if (process.env.README_ROUTES && reply.available === false && Object.keys(reply).length === 3) console.log(`  fallback ${url.pathname}`);
    return reply;
  };
}

const coopEntry = { ...library.find((game) => game.name === film.coopGame), uuid: 'turbo-critters-coop', name: `${film.coopGame} (Co-op)` };
const idle = { apps: library };
const streaming = { apps: library, streams: [stream], app: film.playing };
// A second paired device joined the running game as a guest, and a third asks to.
const guests = { ...streaming, guests: {
  guests: [{ id: 1, name: 'Laptop', permission: 'controller', streaming: true, since: now - 1_260, expires: null }],
  waiting: [{ id: 2, name: 'Pixel 9 Pro', seconds_left: 48 }],
} };
const openPanel = (title) => async (page) => {
  await page.locator(`details:has(> summary .title:text-is("${title}")) > summary`).click();
};
// Each still: the console page, the mocked state and what to open before the picture.
const stills = {
  home: { path: '/', scene: idle },
  // Home while a device streams: the live stage.
  stream: { path: '/', scene: streaming },
  games: { path: '/library', scene: idle },
  game: {
    path: '/library/ashen-crown', scene: idle,
    act: async (page) => {
      await openPanel('Graphics driver')(page);
      await page.evaluate(() => window.scrollTo(0, 0));
    },
  },
  'add-games': {
    path: '/library/import', scene: idle,
    act: async (page) => {
      for (const name of ['Redline Avenue', 'Skyline Strikers', 'Tidebound']) {
        await page.getByRole('checkbox', { name: new RegExp(`^Add ${name} from`) }).check();
      }
      await page.evaluate(() => window.scrollTo(0, 0));
    },
  },
  settings: { path: '/settings', scene: idle },
  devices: { path: '/devices', scene: idle },
  sessions: { path: '/sessions', scene: idle },
  guests: { path: '/', scene: guests, top: 'section.panel:has(:text-is("Guests"))' },
  clips: { path: '/clips', scene: streaming },
  displays: { path: '/troubleshoot', scene: idle },
  coop: { path: '/', scene: { apps: [...library, coopEntry], streams: film.coopStreams, app: coopEntry.name, coop: film.coop },
    top: 'section.panel:has(ul.seats)' },
};

async function open(browser, base, { width, height, scale, theme }, state) {
  // English dates and numbers, whatever the capturing PC's language.
  const context = await browser.newContext({ viewport: { width, height }, deviceScaleFactor: scale, colorScheme: theme, locale: 'en-US' });
  await context.addInitScript((value) => {
    try { localStorage.setItem('butterpollo.theme', value); } catch { /* the console falls back to system */ }
  }, theme);
  const page = await context.newPage();
  // Vite compiles a page's modules on first use, which can take a while under parallel build load.
  page.setDefaultTimeout(90000);
  const errors = [];
  page.on('pageerror', (error) => errors.push(process.env.TOUR_DEBUG ? error.stack : error.message));
  const reply = answer(state);
  await page.route('**/*', async (route) => {
    const url = new URL(route.request().url());
    if (url.origin !== base) {
      errors.push(`blocked a request outside the console: ${url.origin}`);
      return route.abort();
    }
    if (!url.pathname.startsWith('/api/')) return route.continue();
    if (route.request().method() !== 'GET') {
      // Devices asks for a pairing code as it opens: the film's pattern, which encodes nothing and has no address.
      if (url.pathname === '/api/pairing/qr') {
        return route.fulfill({ json: { ...film.pairingCodes, codes: film.pairingCodes.codes.map((code) => ({ ...code, address: '' })) } });
      }
      return route.fulfill({ json: { status: true } });
    }
    const cover = /^\/api\/apps\/([a-z0-9-]+)\/cover$/.exec(url.pathname);
    if (cover) {
      const file = path.join(film.covers, `${cover[1].replace(/-coop$/, '')}.webp`);
      if (!fs.existsSync(file)) return route.fulfill({ status: 404, body: '' });
      return route.fulfill({ contentType: 'image/webp', body: fs.readFileSync(file) });
    }
    return route.fulfill({ json: reply(url) });
  });
  return { context, page, errors };
}

async function shootStills(browser, base) {
  const dir = path.join(out, 'stills');
  fs.mkdirSync(dir, { recursive: true });
  for (const theme of ['dark', 'light']) {
    for (const [name, still] of Object.entries(stills)) {
      // README_STILLS=home,games takes only those again.
      if (process.env.README_STILLS && !process.env.README_STILLS.split(',').includes(name)) continue;
      const state = { scene: still.scene };
      const { context, page, errors } = await open(browser, base, { width: 1440, height: 900, scale: 2, theme }, state);
      await page.goto(base + still.path);
      await page.waitForLoadState('networkidle');
      await page.addStyleTag({ content: film.HIDE_VERSION });
      if (still.act) await still.act(page);
      // `top` names what the picture starts at, `margin` pixels below the window's edge (28 unless set).
      if (still.top) {
        await page.locator(still.top).first().evaluate((node, margin) => scrollTo(0, node.getBoundingClientRect().top + scrollY - margin), still.margin ?? 28);
      }
      // Park the pointer in the top-left corner outside any nav link so nothing shows a hover ring.
      await page.mouse.move(4, 4);
      await page.waitForTimeout(900);
      const file = path.join(dir, `${name}-${theme}.png`);
      await page.screenshot({ path: file, fullPage: !!process.env.README_FULL });
      await context.close();
      if (errors.length) throw new Error(`${name}: ${errors.join('; ')}`);
      console.log(`Wrote ${file}`);
    }
  }
}

// A pointer drawn into the page: a headless browser has none, and the tour is easier to follow with one.
const POINTER = () => {
  const dot = document.createElement('div');
  dot.id = 'tour-pointer';
  dot.style.cssText = 'position:fixed;left:0;top:0;width:22px;height:22px;margin:-11px 0 0 -11px;border-radius:50%;'
    + 'background:rgba(255,255,255,.28);border:2px solid rgba(255,255,255,.9);box-shadow:0 1px 6px rgba(0,0,0,.5);'
    + 'pointer-events:none;z-index:2147483647;transition:transform .12s ease;will-change:left,top';
  const place = () => document.body && !dot.isConnected && document.body.append(dot);
  document.addEventListener('DOMContentLoaded', place);
  place();
  addEventListener('mousemove', (event) => { place(); dot.style.left = `${event.clientX}px`; dot.style.top = `${event.clientY}px`; }, true);
  addEventListener('mousedown', () => { dot.style.transform = 'scale(.7)'; }, true);
  addEventListener('mouseup', () => { dot.style.transform = ''; }, true);
};

async function shootTour(browser, base) {
  const dir = path.join(out, 'tour');
  fs.rmSync(dir, { recursive: true, force: true });
  fs.mkdirSync(dir, { recursive: true });
  const size = { width: 1280, height: 800 };
  const state = { scene: idle };
  const { context, page, errors } = await open(browser, base, { ...size, scale: 1, theme: 'dark' }, state);
  await context.addInitScript(POINTER);
  await page.goto(`${base}/`);
  await page.waitForLoadState('networkidle');
  await page.addStyleTag({ content: film.HIDE_VERSION });
  let at = { x: 640, y: 620 };
  await page.mouse.move(at.x, at.y);
  await page.waitForTimeout(800);

  // Chrome hands over a frame whenever the page changes; frames.txt keeps how long each one stayed.
  const cdp = await context.newCDPSession(page);
  const frames = [];
  cdp.on('Page.screencastFrame', async (frame) => {
    const name = `frame-${String(frames.length).padStart(5, '0')}.jpg`;
    frames.push({ name, time: frame.metadata.timestamp });
    fs.writeFileSync(path.join(dir, name), Buffer.from(frame.data, 'base64'));
    await cdp.send('Page.screencastFrameAck', { sessionId: frame.sessionId }).catch(() => {});
  });
  await cdp.send('Page.startScreencast', { format: 'jpeg', quality: 96, maxWidth: size.width, maxHeight: size.height, everyNthFrame: 1 });

  const pause = (ms) => page.waitForTimeout(ms);
  const glide = async (x, y, ms = 550) => {
    const steps = Math.max(1, Math.round(ms / 16));
    for (let i = 1; i <= steps; i++) {
      const p = i / steps;
      const e = p * p * (3 - 2 * p);
      await page.mouse.move(at.x + (x - at.x) * e, at.y + (y - at.y) * e);
      await pause(16);
    }
    at = { x, y };
  };
  const click = async (locator, wait = 1500) => {
    const target = locator.first();
    await target.scrollIntoViewIfNeeded();
    const box = await target.boundingBox();
    if (!box) throw new Error('the tour lost an element it clicks');
    await glide(box.x + box.width / 2, box.y + box.height / 2);
    await pause(140);
    await page.mouse.down();
    await pause(90);
    await page.mouse.up();
    await pause(wait);
  };
  const scroll = async (top, ms = 900) => {
    await page.evaluate(([y]) => (document.scrollingElement || document.documentElement).scrollTo({ top: y, behavior: 'smooth' }), [top]);
    await pause(ms);
  };
  // Scrolls until `selector` starts `margin` pixels below the window's top edge.
  const scrollToNode = async (selector, margin, ms = 900) => {
    const top = await page.locator(selector).first().evaluate((node, m) => node.getBoundingClientRect().top + scrollY - m, margin);
    await scroll(Math.max(0, top), ms);
  };
  const nav = (name) => page.locator('aside').getByRole('link', { name, exact: true });
  const driver = 'details:has(> summary .title:text-is("Graphics driver"))';

  // Home, the library, one game's driver settings, Settings, the command palette, the session history, then the
  // clips and Home with a stream running and its live stage. Devices is left out: it opens on a pairing code.
  await pause(1600);
  await click(nav('Games'), 1900);
  await click(page.getByRole('link', { name: /Ashen Crown/ }), 1700);
  await scrollToNode(driver, 260, 1000);
  await click(page.locator(`${driver} > summary`), 500);
  await scrollToNode(driver, 40, 900);
  await pause(1700);
  await click(nav('Settings'), 900);
  await scrollToNode('h2:text-is("Picture")', 28, 900);
  await pause(1300);
  await page.keyboard.press('Control+k');
  await pause(700);
  await page.keyboard.type('hdr', { delay: 170 });
  await pause(1700);
  await page.keyboard.press('Escape');
  await pause(500);
  await click(page.locator('aside').getByText('Advanced', { exact: true }), 700);
  await click(nav('Sessions'), 2100);
  state.scene = streaming;
  await click(nav('Clips'), 2300);
  await click(nav('Home'), 3600);

  await cdp.send('Page.stopScreencast');
  await pause(200);
  const end = frames.length ? frames[frames.length - 1].time + 0.4 : 0;
  const list = frames.map((frame, i) => {
    const next = i + 1 < frames.length ? frames[i + 1].time : end;
    return `file '${frame.name}'\nduration ${Math.max(0.001, next - frame.time).toFixed(4)}`;
  });
  // The concat demuxer needs the last file named twice to honour its duration.
  if (frames.length) list.push(`file '${frames[frames.length - 1].name}'`);
  fs.writeFileSync(path.join(dir, 'frames.txt'), `${list.join('\n')}\n`);
  await context.close();
  if (errors.length) throw new Error(`tour: ${errors.join('; ')}`);
  console.log(`Wrote ${frames.length} frames to ${dir}`);
}

(async () => {
  const { base, browser, close } = await film.serve();
  try {
    if (what !== 'tour') await shootStills(browser, base);
    if (what !== 'stills') await shootTour(browser, base);
  } finally {
    await close();
  }
})().catch((error) => {
  console.error(error);
  process.exit(1);
});
