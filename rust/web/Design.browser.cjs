// Keyboard behaviour of the core components, on the dev-only gallery (design.html).
// Uses the optional Playwright runtime like Console.browser.cjs; no host involved.
const assert = require('node:assert/strict');
const { before, after, test } = require('node:test');
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
