// Renders prototype screens to shots/*.png at deviceScaleFactor 2 with installed Chrome.
// node shoot.mjs [name ...]   (no names = all)
import { chromium } from 'playwright-core';
import { resolve, dirname } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
const here = dirname(fileURLToPath(import.meta.url));
const PHONE = [412, 915]; const HH = [1280, 720]; const TV = [1920, 1080]; const DESK = [1280, 800];
const screens = {
  'phone-home': ['phone-home', PHONE], 'phone-game-sheet': ['phone-game', PHONE], 'phone-pcs': ['phone-pcs', PHONE], 'phone-settings': ['phone-settings', PHONE],
  'handheld-home': ['handheld-home', HH],
  'tv-home': ['tv-home', TV], 'tv-game': ['tv-game', TV], 'tv-stream-menu': ['tv-stream', TV],
  'desktop-home': ['desktop-home', DESK], 'desktop-settings': ['desktop-settings', DESK],
  // Defined fallbacks and states
  'phone-pair': ['phone-pair', PHONE],
  'variant-tv-home-baked': ['tv-home', TV, 'tier=baked'], 'variant-tv-stream-menu-fallback': ['tv-stream', TV, 'tier=baked'], 'variant-tv-stream-menu-window': ['tv-stream', TV, 'window=1'], 'stress-tv-stream-window-white': ['tv-stream', TV, 'window=1&stress=1'], 'variant-phone-home-baked': ['phone-home', PHONE, 'tier=baked'],
  'variant-phone-home-hc': ['phone-home', PHONE, 'hc=1'], 'variant-tv-home-hc': ['tv-home', TV, 'hc=1'], 'variant-phone-sheet-hc': ['phone-game', PHONE, 'hc=1'],
  'variant-phone-sheet-de': ['phone-game', PHONE, 'lang=de'], 'variant-phone-settings-de': ['phone-settings', PHONE, 'lang=de'], 'variant-tv-game-de': ['tv-game', TV, 'lang=de'],
  // Every host: first run, the PC list, the app grid, connecting and errors
  'phone-first-run': ['phone-first-run', PHONE], 'phone-first-run-found': ['phone-first-run', PHONE, 'found=1'],
  'tv-pcs': ['tv-pcs', TV], 'desktop-pcs': ['desktop-pcs', DESK],
  'phone-apps': ['phone-apps', PHONE], 'tv-apps': ['tv-apps', TV], 'desktop-apps': ['desktop-apps', DESK],
  'phone-connecting': ['phone-connecting', PHONE], 'phone-launching': ['phone-connecting', PHONE, 'stage=launch'], 'phone-error': ['phone-error', PHONE],
  'variant-tv-home-pad': ['tv-home', TV, 'input=pad'],
  // Themes
  'day-phone-home': ['phone-home', PHONE, 'theme=day'], 'day-phone-game-sheet': ['phone-game', PHONE, 'theme=day'], 'day-phone-settings': ['phone-settings', PHONE, 'theme=day'], 'day-tv-home': ['tv-home', TV, 'theme=day'], 'day-phone-pcs': ['phone-pcs', PHONE, 'theme=day'], 'day-phone-apps': ['phone-apps', PHONE, 'theme=day'],
  'oled-phone-home': ['phone-home', PHONE, 'theme=oled'],
  'stress-day-phone-home': ['phone-home', PHONE, 'theme=day&stress=1&tier=live'], 'stress-day-phone-sheet': ['phone-game', PHONE, 'theme=day&stress=1&tier=live'], 'stress-day-tv-home': ['tv-home', TV, 'theme=day&stress=1&tier=live'],
  'readme-banner': ['banner', [1280, 400]], 'stress-tv-stream-white': ['tv-stream', TV, 'stress=1&tier=live'],
  'stress-phone-home-white': ['phone-home', PHONE, 'stress=1&tier=live'], 'stress-phone-sheet-white': ['phone-game', PHONE, 'stress=1&tier=live'], 'stress-tv-game-white': ['tv-game', TV, 'stress=1&tier=live'],
};
const only = process.argv.slice(2);
const browser = await chromium.launch({ channel: 'chrome', args: ['--enable-gpu-rasterization', '--force-color-profile=srgb'] });
for (const [name, [file, [w, h], q]] of Object.entries(screens)) {
  if (only.length && !only.some((o) => name.startsWith(o))) continue;
  const ctx = await browser.newContext({ viewport: { width: w, height: h }, deviceScaleFactor: 2 });
  const page = await ctx.newPage();
  page.on('pageerror', (e) => console.error(name, e.message));
  const url = pathToFileURL(resolve(here, file + '.html')).href + (q ? '?' + q : '');
  await page.goto(url);
  await page.evaluate(() => document.fonts.ready);
  await page.waitForLoadState('networkidle');
  await page.waitForTimeout(150);
  const over = await page.evaluate(() => {
    const out = []; const vw = innerWidth;
    document.querySelectorAll('.row,.ctl,.play,.tab,.chip,.seg>span,.g-c button').forEach((el) => {
      if (el.scrollWidth > el.clientWidth + 1) out.push('overflow:' + el.textContent.trim().slice(0, 30));
      const r = el.getBoundingClientRect();
      if (r.height && r.height < 31.9) out.push('small:' + el.textContent.trim().slice(0, 30) + ' ' + r.height.toFixed(0));
    });
    return out;
  });
  if (over.length) console.log(name, [...new Set(over)].join(' | '));
  await page.screenshot({ path: resolve(here, 'shots', name + '.png') });
  await ctx.close();
}
await browser.close();
