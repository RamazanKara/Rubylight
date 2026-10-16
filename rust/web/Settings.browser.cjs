// Uses the optional Playwright runtime from PathPicker.browser.cjs, without a host process.
const assert = require('node:assert/strict');
const { before, after, test } = require('node:test');
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || 'playwright');
const path = require('node:path');

let server, browser, base;
before(async () => {
  const { createServer } = await import('vite');
  const { svelte } = await import('@sveltejs/vite-plugin-svelte');
  server = await createServer({
    root: __dirname, configFile: false, plugins: [svelte()],
    server: { host: '127.0.0.1', port: 0, hmr: false },
  });
  await server.listen();
  base = `http://127.0.0.1:${server.httpServer.address().port}`;
  browser = await chromium.launch({ headless: true,
    ...(process.env.CHROMIUM_EXECUTABLE ? { executablePath: process.env.CHROMIUM_EXECUTABLE } : {}),
  });
});
after(async () => { await browser?.close(); await server?.close(); });

async function fixture(t, initial = {}, metadata = {}) {
  const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
  page.setDefaultTimeout(10000);
  const errors = [], writes = [];
  let config = { ...initial };
  page.on('pageerror', error => errors.push(error.message));
  page.on('console', message => { if (['error', 'warning'].includes(message.type())) errors.push(message.text()); });
  t.after(async () => {
    assert.equal(await page.title(), 'Rubylight');
    assert.equal(await page.locator('vite-error-overlay').count(), 0);
    await page.close();
    assert.deepEqual(errors, []);
  });
  await page.route('**/*', async route => {
    const request = route.request(), url = new URL(request.url());
    assert.equal(url.origin, base, 'the fixture must not contact a real host');
    if (!url.pathname.startsWith('/api/')) return route.continue();
    const reply = body => route.fulfill({ json: body });
    if (url.pathname === '/api/auth/status') return reply({ credentials_configured: true, authenticated: true });
    if (url.pathname === '/api/csrf-token') return reply({ csrf_token: 'fixture-token' });
    if (url.pathname === '/api/clients/pending') return reply({ requests: [] });
    if (url.pathname === '/api/metadata') return reply({
      version: 'test', host_name: 'Settings fixture', pc_address: '127.0.0.1',
      audio_sinks: [], gpus: [], ...metadata,
    });
    if (url.pathname === '/api/config') {
      if (request.method() === 'GET') return reply(config);
      assert.equal(request.method(), 'PATCH');
      assert.equal(request.headers()['x-csrf-token'], 'fixture-token');
      const body = request.postDataJSON();
      writes.push(body);
      for (const [key, value] of Object.entries(body)) {
        if (value === null) delete config[key];
        else config[key] = value;
      }
      return reply({ status: true, restart_required: false });
    }
    assert.fail(`unexpected API request: ${url.pathname}`);
  });
  return { page, writes };
}

const setting = (page, key) => page.locator(`[data-setting="${key}"]`);
const search = (page, text) => page.getByRole('searchbox', { name: 'Search settings' }).fill(text);
async function save(page) {
  const response = page.waitForResponse(response => response.url().endsWith('/api/config') && response.request().method() === 'PATCH');
  await page.getByRole('button', { name: 'Save changes', exact: true }).click();
  await response;
}

test('numeric errors block saves, survive categories, and can be corrected without losing focus', async t => {
  const { page, writes } = await fixture(t);
  await page.goto(base + '/settings/network');
  await setting(page, 'port').getByRole('spinbutton').fill('70000');
  assert.equal(await page.getByRole('button', { name: 'Save changes' }).isDisabled(), true);
  await page.getByRole('link', { name: /^General/ }).click();
  await page.getByRole('button', { name: 'Review errors', exact: true }).click();
  const input = setting(page, 'port').getByRole('spinbutton');
  assert.equal(await input.inputValue(), '70000');
  assert.equal(await input.getAttribute('aria-invalid'), 'true');
  if (process.env.BUTTERPOLLO_TEST_ARTIFACTS) {
    await page.screenshot({ path: path.join(process.env.BUTTERPOLLO_TEST_ARTIFACTS, 'settings-errors-desktop.png') });
    await page.setViewportSize({ width: 390, height: 844 });
    assert(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
    await page.screenshot({ path: path.join(process.env.BUTTERPOLLO_TEST_ARTIFACTS, 'settings-errors-mobile.png'), fullPage: true });
  }
  await input.fill('47999.5');
  assert.equal(await page.getByRole('button', { name: 'Save changes' }).isDisabled(), true);
  await page.getByText('Enter a whole number.', { exact: true }).waitFor();
  await input.fill('47999');
  assert.equal(await input.evaluate(node => node === document.activeElement), true);
  assert.equal(await page.getByRole('button', { name: 'Save changes' }).isEnabled(), true);
  assert.deepEqual(writes, []);
  await save(page);
  assert.deepEqual(writes, [{ port: 47999 }]);
});

test('an incomplete number cannot silently reset a setting and undo restores its saved value', async t => {
  const { page, writes } = await fixture(t, { port: 48123 });
  await page.goto(base + '/settings/network');
  const input = setting(page, 'port').getByRole('spinbutton');
  await input.fill('');
  await input.pressSequentially('1e');
  assert.equal(await input.getAttribute('aria-invalid'), 'true');
  assert.equal(await page.getByRole('button', { name: 'Save changes' }).isDisabled(), true);
  await page.getByRole('link', { name: /^General/ }).click();
  await page.getByRole('button', { name: 'Review errors', exact: true }).click();
  assert.equal(await input.getAttribute('aria-invalid'), 'true');
  await setting(page, 'port').getByRole('button', { name: /^Undo/ }).click();
  assert.equal(await input.inputValue(), '48123');
  assert.equal(await page.getByRole('button', { name: 'Save changes' }).count(), 0);
  assert.deepEqual(writes, []);
  await input.fill('');
  await save(page);
  assert.deepEqual(writes, [{ port: null }]);
});

test('JSON errors remain recoverable when an advanced setting is no longer on screen', async t => {
  const { page, writes } = await fixture(t);
  await page.goto(base + '/settings/display');
  await search(page, 'dd_mode_remapping');
  const input = setting(page, 'dd_mode_remapping').getByRole('textbox');
  await input.fill('{broken');
  await page.getByRole('link', { name: /^General/ }).click();
  await page.getByRole('button', { name: 'Review errors', exact: true }).click();
  assert.equal(await input.inputValue(), '{broken');
  assert.equal(await page.getByRole('button', { name: 'Save changes' }).isDisabled(), true);
  await input.fill('{"mixed": []}');
  assert.equal(await input.evaluate(node => node === document.activeElement), true);
  await save(page);
  assert.deepEqual(writes, [{ dd_mode_remapping: { mixed: [] } }]);
});

test('search finds menu choices and category context, and explains hidden dependencies', async t => {
  const { page } = await fixture(t, { controller: false, adapter_name: 'Unavailable GPU' });
  await page.goto(base + '/settings/general');
  await search(page, 'input DualShock 4');
  await setting(page, 'gamepad').waitFor();
  assert.match(await setting(page, 'gamepad').innerText(), /Turn on Controllers/);
  assert.equal(await setting(page, 'gamepad').getByRole('combobox').locator('option', { hasText: 'DualShock 4' }).count(), 1);
  await search(page, 'adapter_name');
  assert.match(await setting(page, 'adapter_name').getByRole('combobox').innerText(), /Saved custom choice: Unavailable GPU/);
  await search(page, 'amd quality');
  await setting(page, 'amd_quality').waitFor();
  assert.equal(await setting(page, 'amd_quality').locator('.dependency').count(), 0, 'AMF settings are in effect, not conditional');
  if (process.env.BUTTERPOLLO_TEST_ARTIFACTS) {
    await search(page, 'AMD quality preset');
    await page.screenshot({ path: path.join(process.env.BUTTERPOLLO_TEST_ARTIFACTS, 'settings-search-desktop.png'), fullPage: true });
    await page.setViewportSize({ width: 390, height: 844 });
    assert(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
    await page.screenshot({ path: path.join(process.env.BUTTERPOLLO_TEST_ARTIFACTS, 'settings-search-mobile.png'), fullPage: true });
  }
});

test('rows show the default only when it differs, and config keys only on request', async t => {
  const { page, writes } = await fixture(t, { fec_percentage: 30, adaptive_fec: 'enabled' });
  await page.goto(base + '/settings/video');
  const fec = setting(page, 'fec_percentage');
  assert.match(await fec.innerText(), /Default: 20%/);
  await fec.getByRole('button', { name: 'Reset to default for Error correction', exact: true }).waitFor();
  // Stored, but the same as the default: nothing to reset.
  assert.doesNotMatch(await setting(page, 'adaptive_fec').innerText(), /Default|Reset/);
  assert.doesNotMatch(await setting(page, 'max_bitrate').innerText(), /Default|Reset/);
  // No developer noise by default; the key is in the label's tooltip.
  assert.equal(await page.locator('[data-setting] code').count(), 0);
  assert.equal(await page.getByText('Leave blank to use the default.').count(), 0);
  assert.equal(await fec.locator('[title="Config key: fec_percentage"]').count(), 1);
  assert.match(await setting(page, 'pyrowave').innerText(), /Applies after a restart/);
  assert.doesNotMatch(await setting(page, 'frame_pacing').innerText(), /Applies after a restart/);
  // An empty number field uses the default, and says so.
  const input = fec.getByRole('spinbutton');
  await input.fill('');
  assert.equal(await input.getAttribute('placeholder'), 'Default: 20');
  await save(page);
  assert.deepEqual(writes, [{ fec_percentage: null }]);
  assert.equal(await input.inputValue(), '20');
  assert.doesNotMatch(await fec.innerText(), /Default|Reset/);
  // The keys switch shows every key and is remembered in this browser.
  await page.getByRole('switch', { name: 'Show config keys', exact: true }).check();
  await fec.locator('code', { hasText: 'fec_percentage' }).waitFor();
  await page.reload();
  await setting(page, 'fec_percentage').locator('code', { hasText: 'fec_percentage' }).waitFor();
  assert.equal(await page.getByRole('switch', { name: 'Show config keys', exact: true }).isChecked(), true);
  await page.getByRole('switch', { name: 'Show config keys', exact: true }).uncheck();
  assert.equal(await page.locator('[data-setting] code').count(), 0);
});

test('search shows the key it matched and finds facts behind More', async t => {
  const { page } = await fixture(t);
  await page.goto(base + '/settings/general');
  await search(page, 'fec_percentage');
  await setting(page, 'fec_percentage').locator('code', { hasText: 'fec_percentage' }).waitFor();
  await search(page, '1593 Mbps');
  const pyrowave = setting(page, 'pyrowave');
  await pyrowave.waitFor();
  const fact = pyrowave.getByText(/1593 Mbps for 4K60/);
  assert.equal(await fact.isVisible(), false);
  await pyrowave.locator('summary', { hasText: 'More' }).click();
  assert.equal(await fact.isVisible(), true);
});

test('controls share a right-hand column on wide screens and stack under the text on phones', async t => {
  const { page } = await fixture(t);
  await page.goto(base + '/settings/video');
  const box = (row, locator) => setting(page, row).locator(locator).first().boundingBox();
  const label = await box('frame_pacing', 'label');
  const select = await box('frame_pacing', 'select');
  const number = await box('fec_percentage', 'input');
  const toggle = await box('pyrowave', '.switch');
  assert(select.x > label.x + 300, 'the select sits to the right of its label');
  assert(Math.abs(select.width - 320) <= 1 && Math.abs(number.width - 320) <= 1, 'selects and numbers share one width');
  assert(Math.abs(select.x - number.x) <= 1, 'selects and numbers line up');
  assert(Math.abs(toggle.x + toggle.width - (select.x + select.width)) <= 1, 'switches are right-aligned in the column');
  await page.setViewportSize({ width: 390, height: 844 });
  const narrowLabel = await box('frame_pacing', 'label');
  const narrowSelect = await box('frame_pacing', 'select');
  assert(narrowSelect.y > narrowLabel.y + narrowLabel.height, 'the select moves under its label');
  const switchLabel = await box('pyrowave', 'label');
  const narrowToggle = await box('pyrowave', '.switch');
  assert(narrowToggle.y < switchLabel.y + switchLabel.height, 'a switch stays beside its label');
  assert(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
});

test('discarding asks first, and the save bar stays in view on a phone', async t => {
  const { page, writes } = await fixture(t);
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(base + '/settings/general');
  const name = page.getByLabel('Host name', { exact: true });
  await name.fill('Draft name');
  await page.evaluate(() => scrollTo(0, 0));
  const bar = await page.getByRole('button', { name: 'Save changes', exact: true }).boundingBox();
  assert(bar && bar.y + bar.height <= 844, 'the save bar is visible without scrolling');
  await page.getByText('1 unsaved change', { exact: true }).waitFor();
  await page.getByRole('button', { name: 'Discard', exact: true }).click();
  const dialog = page.getByRole('dialog');
  await dialog.getByText(/1 unsaved change is lost/).waitFor();
  await dialog.getByRole('button', { name: 'Cancel', exact: true }).click();
  assert.equal(await name.inputValue(), 'Draft name');
  await page.getByRole('button', { name: 'Discard', exact: true }).click();
  await dialog.getByRole('button', { name: 'Discard', exact: true }).click();
  assert.equal(await name.inputValue(), '');
  assert.equal(await page.getByRole('button', { name: 'Save changes' }).count(), 0);
  assert.deepEqual(writes, []);
});

test('a malformed fixed resolution blocks saving and an empty one warns', async t => {
  const { page, writes } = await fixture(t);
  await page.goto(base + '/settings/display');
  await setting(page, 'dd_resolution_option').getByRole('combobox').selectOption('manual');
  const row = setting(page, 'dd_manual_resolution');
  await row.getByText(/Required: streams fail until this has a value/).waitFor();
  const input = row.getByRole('textbox');
  await input.fill('2560*1440');
  assert.equal(await input.getAttribute('aria-invalid'), 'true');
  await row.getByText('Enter WIDTHxHEIGHT, for example 2560x1440.', { exact: true }).waitFor();
  assert.equal(await page.getByRole('button', { name: 'Save changes' }).isDisabled(), true);
  await input.fill('2560x1440');
  assert.equal(await row.getByText(/Required/).count(), 0);
  await save(page);
  assert.deepEqual(writes, [{ dd_resolution_option: 'manual', dd_manual_resolution: '2560x1440' }]);
});

test("the AMD AMF settings are shown whatever the encoder setting or this PC's GPU is", async t => {
  const gpu = (name, vendor) => ({ name, vendor, dedicated_memory: 0, luid: [1, 0], pnp_id: null });
  const { page } = await fixture(t, {}, { gpus: [gpu('NVIDIA GeForce RTX 4090', 0x10de)] });
  await page.goto(base + '/settings/encoders');
  const usage = setting(page, 'amd_usage');
  await usage.waitFor();
  assert.equal(await usage.locator('.dependency').count(), 0, 'AMF settings are in effect, not conditional');
  assert.equal(await setting(page, 'nvenc_preset').count(), 0);
  assert.equal(await setting(page, 'qsv_preset').count(), 0);
  assert.equal(await setting(page, 'sw_preset').count(), 0);
  assert.deepEqual(await setting(page, 'encoder').getByRole('combobox').locator('option').allInnerTexts(),
    ['Automatic', 'AMD AMF', 'PyroWave (automatic for other codecs)']);
  if (process.env.BUTTERPOLLO_TEST_ARTIFACTS) {
    await page.screenshot({ path: path.join(process.env.BUTTERPOLLO_TEST_ARTIFACTS, 'settings-encoders-amd-desktop.png'), fullPage: true });
  }
  // Choosing AMF or PyroWave keeps them.
  for (const encoder of ['amf', 'pyrowave']) {
    await setting(page, 'encoder').getByRole('combobox').selectOption(encoder);
    await usage.waitFor();
  }
});

test('undoing an advanced change keeps the Advanced section open', async t => {
  const { page } = await fixture(t);
  await page.goto(base + '/settings/video');
  const advanced = page.locator('details.advanced');
  await advanced.locator(':scope > summary').click();
  const row = setting(page, 'gpu_compute_conversion');
  await row.getByRole('switch').press('Space');
  await row.getByRole('button', { name: /^Undo/ }).click();
  assert.equal(await page.getByRole('button', { name: 'Save changes' }).count(), 0);
  assert.equal(await advanced.evaluate(node => node.open), true, 'the section the user is working in must not collapse');
  await row.getByRole('switch').waitFor();
});

test('editing a dependency does not hide unsaved child settings and review keeps them reachable', async t => {
  const { page } = await fixture(t);
  await page.goto(base + '/settings/general');
  const toggle = page.getByRole('switch', { name: /^Send hardware metrics to client/ });
  await toggle.press('Space');
  const interval = setting(page, 'hardware_metrics_interval').getByRole('spinbutton');
  await interval.fill('750');
  await toggle.press('Space');
  assert.equal(await interval.inputValue(), '750');
  assert.match(await setting(page, 'hardware_metrics_interval').innerText(), /Turn on Send hardware metrics/);
  await page.getByRole('button', { name: 'Review changes', exact: true }).click();
  assert.equal(await interval.inputValue(), '750');
  await setting(page, 'hardware_metrics_interval').getByRole('button', { name: /^Undo/ }).click();
  assert.equal(await interval.inputValue(), '1000');
  assert.equal(await page.getByRole('button', { name: 'Save changes' }).count(), 0);
});
