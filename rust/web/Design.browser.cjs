// Keyboard behaviour of the core components, on the dev-only gallery (design.html).
// Uses the optional Playwright runtime like Console.browser.cjs; no host involved.
const assert = require('node:assert/strict');
const { before, after, test } = require('node:test');
const { warmUp } = require('./browser-warmup.cjs');
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || 'playwright');

let server, browser, base;
before(async () => {
  const { createServer } = await import('vite');
  const { svelte } = await import('@sveltejs/vite-plugin-svelte');
  server = await createServer({ root: __dirname, configFile: false, plugins: [svelte()],
    server: { host: '127.0.0.1', port: 0, hmr: false } });
  await server.listen();
  base = `http://127.0.0.1:${server.httpServer.address().port}`;
  browser = await chromium.launch({ headless: true,
    ...(process.env.CHROMIUM_EXECUTABLE ? { executablePath: process.env.CHROMIUM_EXECUTABLE } : {}) });
  await warmUp(browser, base, ['/design.html']);
});
after(async () => { await browser?.close(); await server?.close(); });

async function gallery(t, options = {}) {
  const page = await browser.newPage({ viewport: { width: 1280, height: 900 }, ...options });
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  t.after(async () => { await page.close(); assert.deepEqual(errors, []); });
  await page.goto(base + '/design.html');
  await page.getByRole('heading', { name: 'Design system' }).waitFor();
  return page;
}

test('tabs move with the arrow keys, skip disabled tabs and keep one tab stop', async t => {
  const page = await gallery(t);
  const list = page.getByRole('tablist', { name: 'Stream settings' });
  await list.getByRole('tab', { name: 'Video' }).focus();
  await page.keyboard.press('ArrowRight');
  const audio = list.getByRole('tab', { name: /Audio/ });
  assert.equal(await audio.getAttribute('aria-selected'), 'true');
  assert.equal(await audio.evaluate(node => node === document.activeElement), true);
  await page.getByText('The audio panel.', { exact: false }).waitFor();
  await page.keyboard.press('ArrowRight');
  await page.keyboard.press('ArrowRight');
  assert.equal(await list.getByRole('tab', { name: 'Video' }).getAttribute('aria-selected'), 'true', 'USB is disabled');
  await page.keyboard.press('End');
  assert.equal(await list.getByRole('tab', { name: 'Input' }).getAttribute('aria-selected'), 'true');
  assert.equal(await list.locator('[tabindex="0"]').count(), 1);
});

test('a dialog takes focus, closes on Escape and gives focus back', async t => {
  const page = await gallery(t);
  const opener = page.getByRole('button', { name: 'Open dialog' });
  await opener.click();
  const dialog = page.getByRole('dialog', { name: 'Unpair Pixel 9?' });
  await dialog.waitFor();
  assert.equal(await dialog.evaluate(node => node.contains(document.activeElement)), true);
  await page.keyboard.press('Escape');
  await dialog.waitFor({ state: 'hidden' });
  assert.equal(await opener.evaluate(node => node === document.activeElement), true);
});

test('reduced motion stops pulsing dots and skeletons', async t => {
  const page = await gallery(t, { reducedMotion: 'reduce' });
  const iterations = await page.locator('.bone, .streaming .dot').evaluateAll(nodes =>
    nodes.map(node => getComputedStyle(node).animationIterationCount));
  assert.ok(iterations.length > 0);
  assert.deepEqual([...new Set(iterations)], ['1']);
});

for (const colorScheme of ['light', 'dark']) {
  test(`a disabled button is a flat muted well, not a faded variant (${colorScheme})`, async t => {
    const page = await gallery(t, { colorScheme });
    const look = name => page.getByRole('button', { name, exact: true }).evaluate(node => {
      const style = getComputedStyle(node);
      const muted = getComputedStyle(document.documentElement).getPropertyValue('--muted').trim();
      const probe = document.createElement('span');
      probe.style.color = muted;
      document.body.append(probe);
      const mutedRgb = getComputedStyle(probe).color;
      probe.remove();
      return { opacity: style.opacity, background: style.backgroundColor, color: style.color, mutedRgb, cursor: style.cursor };
    });
    const primary = await look('Start stream');
    const disabled = await look('Disabled primary');
    const secondary = await look('Disabled');
    assert.equal(disabled.opacity, '1', 'no washed-out fade');
    assert.notEqual(disabled.background, primary.background, 'the brand fill goes away');
    assert.equal(disabled.color, disabled.mutedRgb);
    assert.equal(disabled.cursor, 'not-allowed');
    assert.equal(disabled.background, secondary.background, 'every variant disables the same way');
  });
}

test('the empty state icon sits in a well sized to the block', async t => {
  const page = await gallery(t);
  const box = await page.locator('.empty .icon').first().boundingBox();
  assert.ok(box && box.width >= 48 && Math.abs(box.width - box.height) < 1, `icon well ${box?.width}x${box?.height}`);
  const svg = await page.locator('.empty .icon svg').first().boundingBox();
  assert.ok(svg && svg.width >= 28, `icon ${svg?.width}`);
});
