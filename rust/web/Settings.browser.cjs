// Uses the optional Playwright runtime from PathPicker.browser.cjs, without a host process.
const assert = require('node:assert/strict');
const { before, after, test } = require('node:test');
const { warmUp } = require('./browser-warmup.cjs');
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
  await warmUp(browser, base, ['/settings/picture']);
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
      capture_status: { displays: [], error: null }, audio_sinks: [], gpus: [], ...metadata,
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
    // Status panels the tabs read: This PC (updates, wake), Picture (integrations) and Advanced (sign-in).
    if (url.pathname === '/api/updates') return reply({ releases: [], checked_at: 0, auto_update: true, update_check_interval: 0,
      install_supported: true, update_available: false, phase: 'idle' });
    if (url.pathname === '/api/power') return reply({ status: true,
      wake: { revision: 1, primary: null, targets: [],
        system: { sleep: 's3', modern_standby: false, hibernate: false, fast_startup: 'off', shutdown_wake: 'unknown' } },
      idle_sleep: { state: 'off', minutes: 0, reason: null, remaining_seconds: null }, last_sleep: null,
      fix: { running: false, changes: [], skipped: [], error: null, fast_startup_off_offered: false, last: null } });
    if (url.pathname === '/api/auth/sessions') return reply({ sessions: [
      { id: 'this', current: true, username: 'fixture-user', created_at: 0, expires_at: 0, refresh_expires_at: 0, last_seen: 0,
        remember_me: true, user_agent: 'Chrome', remote_address: '127.0.0.1', device_label: 'Chrome on Windows' },
    ] });
    if (url.pathname === '/api/health/vulkan-hdr-layer') return reply({ status: true, installed: false, enabled: false, available: true, active: false });
    if (url.pathname === '/api/frame-limiter/status') return reply({ status: true, enabled: false, configured_provider: 'none', active_provider: 'none',
      rtss_available: false, resolved_path: '', path_exists: false, process_running: false, message: '' });
    assert.fail(`unexpected API request: ${url.pathname}`);
  });
  return { page, writes };
}

// All settings shows every setting; only expert groups fold.
async function showAdvanced(page) {
  await page.locator('[data-setting]').first().waitFor();
}
const fold = (page, name) => page.locator('details.disclosure', { has: page.locator(':scope > summary', { hasText: name }) });
/** The (i) beside a setting's label, which opens its finer points. */
const about = row => row.getByRole('button', { name: /^About / });

const setting = (page, key) => page.locator(`[data-setting="${key}"]`);
const search = (page, text) => page.getByRole('searchbox', { name: 'Search settings' }).fill(text);
async function save(page) {
  const response = page.waitForResponse(response => response.url().endsWith('/api/config') && response.request().method() === 'PATCH');
  await page.getByRole('button', { name: 'Save changes', exact: true }).click();
  await response;
}

test('direct driver push can be selected, saved, reloaded and reset to Automatic', async t => {
  const { page, writes } = await fixture(t);
  await page.goto(base + '/settings/picture');
  await showAdvanced(page);
  const row = setting(page, 'capture');
  const control = row.getByRole('combobox');
  assert.equal(await control.inputValue(), 'auto');
  await control.selectOption({ label: 'Direct driver push' });
  await about(row).click();
  assert.match(await row.innerText(), /Rubylight's own virtual display driver/);
  assert.match(await row.innerText(), /falls back to Windows Graphics Capture, then Desktop Duplication/);
  await save(page);
  assert.deepEqual(writes, [{ capture: 'idd-push' }]);
  await page.reload();
  await showAdvanced(page);
  await row.waitFor();
  assert.equal(await control.inputValue(), 'idd-push');
  assert.doesNotMatch(await row.innerText(), /Saved custom choice/);
  assert.equal(await page.getByRole('button', { name: 'Save changes', exact: true }).count(), 0);
  await about(row).click();
  if (process.env.BUTTERPOLLO_TEST_ARTIFACTS) {
    await page.screenshot({ path: path.join(process.env.BUTTERPOLLO_TEST_ARTIFACTS, 'capture-option-desktop.png') });
    await page.setViewportSize({ width: 390, height: 844 });
    await row.scrollIntoViewIfNeeded();
    assert(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
    await page.screenshot({ path: path.join(process.env.BUTTERPOLLO_TEST_ARTIFACTS, 'capture-option-mobile.png') });
  }
  // The finer points close like any popover, out of the way of the row's own buttons.
  await page.keyboard.press('Escape');
  await row.getByRole('button', { name: 'Reset to default for Capture method', exact: true }).click();
  await save(page);
  assert.deepEqual(writes, [{ capture: 'idd-push' }, { capture: null }]);
  await page.reload();
  await showAdvanced(page);
  await row.waitFor();
  assert.equal(await control.inputValue(), 'auto');
});

test('numeric errors block saves, survive categories, and can be corrected without losing focus', async t => {
  const { page, writes } = await fixture(t);
  await page.goto(base + '/settings/network');
  await fold(page, 'Ports and addresses').locator(':scope > summary').click();
  await setting(page, 'port').getByRole('spinbutton').fill('70000');
  assert.equal(await page.getByRole('button', { name: 'Save changes' }).isDisabled(), true);
  await page.getByRole('link', { name: /^This PC/ }).click();
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
  await showAdvanced(page);
  const input = setting(page, 'port').getByRole('spinbutton');
  await input.fill('');
  await input.pressSequentially('1e');
  assert.equal(await input.getAttribute('aria-invalid'), 'true');
  assert.equal(await page.getByRole('button', { name: 'Save changes' }).isDisabled(), true);
  await page.getByRole('link', { name: /^This PC/ }).click();
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
  await page.goto(base + '/settings/picture');
  await search(page, 'dd_mode_remapping');
  const input = setting(page, 'dd_mode_remapping').getByRole('textbox');
  await input.fill('{broken');
  await page.getByRole('link', { name: /^This PC/ }).click();
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
  await page.goto(base + '/settings/pc');
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
  await page.goto(base + '/settings/picture');
  await showAdvanced(page);
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
  await showAdvanced(page);
  await setting(page, 'fec_percentage').locator('code', { hasText: 'fec_percentage' }).waitFor();
  assert.equal(await page.getByRole('switch', { name: 'Show config keys', exact: true }).isChecked(), true);
  await page.getByRole('switch', { name: 'Show config keys', exact: true }).uncheck();
  assert.equal(await page.locator('[data-setting] code').count(), 0);
});

test('search shows the key it matched and finds facts behind the (i)', async t => {
  const { page } = await fixture(t);
  await page.goto(base + '/settings/pc');
  await search(page, 'fec_percentage');
  await setting(page, 'fec_percentage').locator('code', { hasText: 'fec_percentage' }).waitFor();
  await search(page, '1593 Mbps');
  const pyrowave = setting(page, 'pyrowave');
  await pyrowave.waitFor();
  const fact = pyrowave.getByText(/1593 Mbps for 4K60/);
  assert.equal(await fact.isVisible(), false);
  await about(pyrowave).click();
  assert.equal(await fact.isVisible(), true);
  await page.keyboard.press('Escape');
  assert.equal(await fact.isVisible(), false);
});

test('controls share a right-hand column on wide screens and stack under the text on phones', async t => {
  const { page } = await fixture(t);
  await page.goto(base + '/settings/picture');
  await showAdvanced(page);
  const box = (row, locator) => setting(page, row).locator(locator).first().boundingBox();
  const label = await box('capture', 'label');
  const select = await box('capture', 'select');
  const number = await box('fec_percentage', 'input');
  const toggle = await box('pyrowave', '.switch');
  assert(select.x > label.x + 300, 'the select sits to the right of its label');
  assert(Math.abs(select.width - 320) <= 1 && Math.abs(number.width - 320) <= 1, 'selects and numbers share one width');
  assert(Math.abs(select.x - number.x) <= 1, 'selects and numbers line up');
  assert(Math.abs(toggle.x + toggle.width - (select.x + select.width)) <= 1, 'switches are right-aligned in the column');
  await page.setViewportSize({ width: 390, height: 844 });
  const narrowLabel = await box('capture', 'label');
  const narrowSelect = await box('capture', 'select');
  assert(narrowSelect.y > narrowLabel.y + narrowLabel.height, 'the select moves under its label');
  const switchLabel = await box('pyrowave', 'label');
  const narrowToggle = await box('pyrowave', '.switch');
  assert(narrowToggle.y < switchLabel.y + switchLabel.height, 'a switch stays beside its label');
  assert(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
});

test('discarding asks first, and the save bar stays in view on a phone', async t => {
  const { page, writes } = await fixture(t);
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(base + '/settings/pc');
  const name = page.getByLabel('PC name', { exact: true });
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
  await page.goto(base + '/settings/picture');
  await setting(page, 'dd_resolution_option').getByRole('combobox').selectOption('manual');
  const row = setting(page, 'dd_manual_resolution');
  await row.getByText(/Required: streams fail until this has a value/).waitFor();
  // Common sizes are menu choices; Custom opens a field for any other.
  assert.equal(await row.getByRole('textbox').count(), 0);
  await row.getByRole('combobox').selectOption({ label: 'Custom' });
  const input = row.getByRole('textbox');
  assert.equal(await input.evaluate(node => node === document.activeElement), true);
  await input.fill('2560*1440');
  assert.equal(await input.getAttribute('aria-invalid'), 'true');
  await row.getByText('Enter WIDTHxHEIGHT, for example 2560x1440.', { exact: true }).waitFor();
  assert.equal(await page.getByRole('button', { name: 'Save changes' }).isDisabled(), true);
  await input.fill('2560x1440');
  assert.equal(await row.getByText(/Required/).count(), 0);
  await save(page);
  assert.deepEqual(writes, [{ dd_resolution_option: 'manual', dd_manual_resolution: '2560x1440' }]);
});

test("with Automatic, the encoder settings of this PC's GPU are shown", async t => {
  const gpu = (name, vendor) => ({ name, vendor, dedicated_memory: 0, luid: [1, 0], pnp_id: null });
  const { page } = await fixture(t, {}, { gpus: [gpu('NVIDIA GeForce RTX 4090', 0x10de)] });
  await page.goto(base + '/settings/picture');
  // NVIDIA NVENC is an expert group: folded until opened.
  await fold(page, 'NVIDIA NVENC').locator(':scope > summary').click();
  const preset = setting(page, 'nvenc_preset');
  await preset.waitFor();
  assert.equal(await preset.locator('.dependency').count(), 0, 'NVENC settings are in effect, not conditional');
  assert.equal(await setting(page, 'amd_usage').count(), 0);
  assert.equal(await setting(page, 'qsv_preset').count(), 0);
  assert.equal(await setting(page, 'sw_preset').count(), 0);
  assert.deepEqual(await setting(page, 'encoder').getByRole('combobox').locator('option').allInnerTexts(),
    ['Automatic', 'AMD AMF', 'NVIDIA NVENC', 'PyroWave (automatic for other codecs)']);
  if (process.env.BUTTERPOLLO_TEST_ARTIFACTS) {
    await page.screenshot({ path: path.join(process.env.BUTTERPOLLO_TEST_ARTIFACTS, 'settings-encoders-nvenc-desktop.png'), fullPage: true });
  }
  // Target quality follows variable bitrate.
  assert.equal(await setting(page, 'nvenc_target_quality').count(), 0);
  await setting(page, 'nvenc_rate_control').getByRole('combobox').selectOption('vbr');
  await setting(page, 'nvenc_target_quality').waitFor();
  // Choosing AMF swaps the groups.
  await setting(page, 'encoder').getByRole('combobox').selectOption('amf');
  await fold(page, 'AMD AMF').locator(':scope > summary').click();
  await setting(page, 'amd_usage').waitFor();
  assert.equal(await setting(page, 'nvenc_preset').count(), 0);
});

test('undoing a change in an expert group keeps the group open', async t => {
  const { page } = await fixture(t);
  await page.goto(base + '/settings/picture');
  const advanced = fold(page, 'Capture troubleshooting');
  await advanced.locator(':scope > summary').click();
  const row = setting(page, 'wgc_direct_encoder_input');
  await row.getByRole('switch').press('Space');
  await row.getByRole('button', { name: /^Undo/ }).click();
  assert.equal(await page.getByRole('button', { name: 'Save changes' }).count(), 0);
  assert.equal(await advanced.evaluate(node => node.open), true, 'the section the user is working in must not collapse');
  await row.getByRole('switch').waitFor();
});

test('editing a dependency does not hide unsaved child settings and review keeps them reachable', async t => {
  const { page } = await fixture(t);
  await page.goto(base + '/settings/pc');
  await showAdvanced(page);
  const toggle = page.getByRole('switch', { name: /^Send hardware metrics to client/ });
  await toggle.press('Space');
  // Named intervals are menu choices; Custom opens the field for any other.
  await setting(page, 'hardware_metrics_interval').getByRole('combobox').selectOption({ label: 'Custom' });
  const interval = setting(page, 'hardware_metrics_interval').getByRole('spinbutton');
  await interval.fill('750');
  await toggle.press('Space');
  assert.equal(await interval.inputValue(), '750');
  assert.match(await setting(page, 'hardware_metrics_interval').innerText(), /Turn on Send hardware metrics/);
  await page.getByRole('button', { name: 'Review changes', exact: true }).click();
  assert.equal(await interval.inputValue(), '750');
  await setting(page, 'hardware_metrics_interval').getByRole('button', { name: /^Undo/ }).click();
  // Back to the saved value, which is a named choice again.
  assert.equal(await setting(page, 'hardware_metrics_interval').getByRole('combobox').inputValue(), '1000');
  assert.equal(await interval.count(), 0);
  assert.equal(await page.getByRole('button', { name: 'Save changes' }).count(), 0);
});


test('six tabs keep only expert settings collapsed, searchable, and customized counts visible', async t => {
  const { page, writes } = await fixture(t, { fec_percentage: 30, amd_usage: 'webcam', custom_setting: 'keep' });
  await page.goto(base + '/settings/video');
  await page.waitForURL(base + '/settings/picture');
  const groups = page.getByRole('navigation', { name: 'Settings tabs' });
  assert.deepEqual((await groups.getByRole('link').allInnerTexts()).map(text => text.trim()), ['This PC', 'Picture', 'Sound & controls', 'Games', 'Network', 'Advanced']);
  // A customized expert group opens by itself; the others stay folded.
  const amf = fold(page, 'AMD AMF');
  assert.equal(await amf.evaluate(node => node.open), true);
  assert.match(await amf.locator(':scope > summary').innerText(), /1 customized/);
  assert.equal(await fold(page, 'Capture troubleshooting').evaluate(node => node.open), false);
  // No second fold: every other setting is on the page.
  assert.equal(await page.locator('main details.advanced').count(), 0);
  assert.equal(await setting(page, 'max_bitrate').isVisible(), true);
  assert.equal(await setting(page, 'fec_percentage').isVisible(), true);
  await search(page, 'fec_percentage');
  await setting(page, 'fec_percentage').getByRole('spinbutton').fill('25');
  await save(page);
  assert.deepEqual(writes, [{ fec_percentage: 25 }]);
  await page.goto(base + '/settings/advanced');
  await page.locator('input[readonly]').filter({ visible: true }).waitFor();
  assert.equal(await page.locator('input[readonly]').inputValue(), 'custom_setting');
  // Older addresses, from bookmarks, docs and other pages, open the tab that now holds them under its own address.
  for (const [legacy, tab, label] of [['general', 'pc', 'This PC'], ['video', 'picture', 'Picture'], ['display', 'picture', 'Picture'],
    ['hdr', 'picture', 'Picture'], ['encoders', 'picture', 'Picture'], ['frame-limiting', 'picture', 'Picture'], ['audio', 'sound', 'Sound & controls'],
    ['input', 'sound', 'Sound & controls'], ['library', 'games', 'Games'], ['commands', 'games', 'Games']]) {
    await page.goto(base + '/settings/' + legacy);
    await page.waitForURL(base + '/settings/' + tab);
    await page.getByRole('heading', { name: label, exact: true }).waitFor();
  }
  assert.deepEqual(writes, [{ fec_percentage: 25 }]);
});

test('the WebRTC server is a plain toggle that says its signalling is unencrypted and reveals its ports when on', async t => {
  const { page, writes } = await fixture(t);
  await page.goto(base + '/settings/network');
  const row = setting(page, 'webrtc_server');
  await row.waitFor();
  // No "Experimental" anywhere in the console.
  assert.match(await row.innerText(), /^WebRTC server\n/);
  assert.doesNotMatch(await row.innerText(), /Experimental/);
  assert.equal(await setting(page, 'webrtc_port').count(), 0);
  await row.getByRole('switch').click();
  await about(row).click();
  assert.match(await row.innerText(), /plain ws:\/\/ on the local network/);
  assert.match(await row.innerText(), /does not hide the traffic/);
  await page.keyboard.press('Escape');
  await showAdvanced(page);
  assert.equal(await setting(page, 'webrtc_port').getByRole('spinbutton').inputValue(), '8000');
  assert.equal(await setting(page, 'webrtc_media_port').getByRole('spinbutton').inputValue(), '8001');
  await setting(page, 'webrtc_port').getByRole('spinbutton').fill('80');
  assert.equal(await page.getByRole('button', { name: 'Save changes' }).isDisabled(), true);
  await setting(page, 'webrtc_port').getByRole('spinbutton').fill('8000');
  if (process.env.BUTTERPOLLO_TEST_ARTIFACTS) {
    await row.scrollIntoViewIfNeeded();
    await page.screenshot({ path: path.join(process.env.BUTTERPOLLO_TEST_ARTIFACTS, 'b3-settings-webrtc-toggle.png') });
  }
  await save(page);
  assert.deepEqual(writes, [{ webrtc_server: true }]);
});

test('number and text settings offer named presets, with Custom for any other value', async t => {
  const { page, writes } = await fixture(t, { update_check_interval: 7200 });
  await page.goto(base + '/settings/pc');
  const row = setting(page, 'update_check_interval');
  const menu = row.getByRole('combobox');
  // A stored value that is none of the presets shows as Custom, with its field.
  assert.equal(await menu.locator('option:checked').innerText(), 'Custom');
  assert.equal(await row.getByRole('spinbutton').inputValue(), '7200');
  assert.deepEqual(await menu.locator('option').allInnerTexts(), ['Every 6 hours', 'Daily', 'Weekly', 'Off', 'Custom']);
  await menu.selectOption({ label: 'Weekly' });
  assert.equal(await row.getByRole('spinbutton').count(), 0);
  await save(page);
  assert.deepEqual(writes, [{ update_check_interval: 604800 }]);
  await page.reload();
  assert.equal(await setting(page, 'update_check_interval').getByRole('combobox').locator('option:checked').innerText(), 'Weekly');
});

test('a link to a setting opens the tab that holds it, under that tab\'s address', async t => {
  const { page } = await fixture(t);
  await page.goto(base + '/settings/general?setting=webrtc_server');
  await page.waitForURL(base + '/settings/network?setting=webrtc_server');
  await setting(page, 'webrtc_server').waitFor();
  assert.equal(await page.getByRole('link', { name: 'Network', exact: true }).getAttribute('aria-current'), 'page');
  // A section address opens that section.
  await page.goto(base + '/settings/pc#sleep-and-wake');
  await setting(page, 'sleep_after_idle_minutes').waitFor();
  await page.waitForFunction(() => {
    const top = document.getElementById('sleep-and-wake').getBoundingClientRect().top;
    return top >= document.querySelector('nav.contents').getBoundingClientRect().bottom && top < 200;
  });
});

test('long tabs list their sections in a bar that stays at the top and follows the page', async t => {
  const { page } = await fixture(t);
  await page.goto(base + '/settings/picture');
  const bar = page.getByRole('navigation', { name: 'Picture sections' });
  await bar.getByRole('link').first().waitFor();
  const names = (await bar.getByRole('link').allInnerTexts()).map(text => text.trim());
  assert.deepEqual(names.slice(0, 3), ['Preset', 'Streamed display', 'Resolution and refresh rate']);
  assert.ok(names.includes('HDR') && names.includes('Frame pacing'), names.join(', '));
  assert.equal(names.at(-1), 'Integrations');
  await bar.getByRole('link', { name: 'HDR', exact: true }).click();
  await page.waitForFunction(() => {
    // Just under the bar, which wraps on a wide page.
    const top = document.getElementById('hdr').getBoundingClientRect().top;
    const bar = document.querySelector('nav.contents').getBoundingClientRect().bottom;
    return top >= bar && top < bar + 40;
  });
  // The bar marks the section at the top once the page settles.
  await page.waitForFunction(() => document.querySelector('nav.contents a[aria-current="location"]')?.textContent.trim() === 'HDR');
  assert.ok(Math.abs((await bar.boundingBox()).y) <= 1, 'the bar sticks to the top');
  // An expert section opens when its link is followed.
  assert.equal(await fold(page, 'Capture troubleshooting').evaluate(node => node.open), false);
  await bar.getByRole('link', { name: 'Capture troubleshooting', exact: true }).click();
  assert.equal(await fold(page, 'Capture troubleshooting').evaluate(node => node.open), true);
  // On a phone the bar sits under the console's own header and scrolls sideways, not the page.
  await page.setViewportSize({ width: 390, height: 844 });
  await page.evaluate(() => scrollTo(0, 2000));
  const header = await page.locator('aside').boundingBox();
  await page.waitForFunction(() => {
    const nav = document.querySelector('nav.contents').getBoundingClientRect();
    return Math.abs(nav.top - document.querySelector('aside').getBoundingClientRect().bottom) <= 2;
  });
  assert.ok((await bar.boundingBox()).y >= header.y + header.height - 2);
  assert(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
});

test('Advanced holds sign-in and security, each card as tall as its content', async t => {
  const { page } = await fixture(t);
  await page.goto(base + '/settings/advanced');
  const browsers = page.locator('section.panel', { has: page.getByRole('heading', { name: 'Signed-in browsers', exact: true }) });
  const password = page.locator('section.panel', { has: page.getByRole('heading', { name: 'Change password', exact: true }) });
  await browsers.getByText('Chrome on Windows').first().waitFor();
  const [a, b] = [await browsers.boundingBox(), await password.boundingBox()];
  assert.ok(Math.abs(a.y - b.y) <= 1, 'side by side');
  assert.ok(Math.abs(a.height - b.height) > 1, 'neither card is stretched to the other');
  await page.getByRole('region', { name: 'Sign-in and security' }).getByRole('link', { name: 'API tokens', exact: true }).waitFor();
  // Never a Windows password field: the console's own password only.
  assert.equal(await page.getByLabel(/Windows/).count(), 0);
});
