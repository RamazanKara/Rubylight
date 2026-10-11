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

async function fixture(t, options = {}) {
  const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
  page.setDefaultTimeout(10000);
  const errors = [], writes = [];
  let pending = options.pending || [];
  let failures = options.failures || 0;
  const metadata = {
    version: '2.2.2', host_name: 'Fixture host', pc_address: '127.0.0.1', pc_addresses: [],
    paired_devices: 1, enable_pairing: options.pairing !== false, warnings: [],
    encoder_status: { state: 'ready', h264: true },
    capture_status: { displays: [], virtual_display_configured: false },
    virtual_display: { capable: false, ready: false }, audio_enabled: false, features: {},
  };
  page.on('pageerror', error => errors.push(error.message));
  t.after(async () => { await page.close(); assert.deepEqual(errors, []); });
  await page.route('**/*', async route => {
    const request = route.request(), url = new URL(request.url());
    assert.equal(url.origin, base, 'the fixture must not contact a real host');
    if (!url.pathname.startsWith('/api/')) return route.continue();
    const reply = body => route.fulfill({ json: body });
    if (request.method() !== 'GET') {
      writes.push(url.pathname);
      if (url.pathname === '/api/pin') pending = [];
      return reply({ status: true, uuid: 'saved-app' });
    }
    if (url.pathname === '/api/auth/status') {
      if (failures-- > 0) return route.fulfill({ status: 503, body: 'Host is restarting' });
      return reply({ status: true, credentials_configured: true, authenticated: true });
    }
    if (url.pathname === '/api/csrf-token') return reply({ csrf_token: 'fixture-token' });
    if (url.pathname === '/api/config') return reply({ sunshine_name: 'Fixture host' });
    if (url.pathname === '/api/metadata') return reply(metadata);
    if (url.pathname === '/api/clients/pending') return reply({ requests: pending });
    if (url.pathname === '/api/clients/list') return reply({ clients: [{ uuid: 'client', name: 'Test device', enabled: true, perm: 0x071f1f00 }] });
    if (url.pathname === '/api/clients/hdr-profiles') return reply({ profiles: [] });
    if (url.pathname === '/api/display-devices') return reply([]);
    if (url.pathname === '/api/apps') return reply({ apps: [] });
    if (url.pathname === '/api/session/status') return reply({ activeSessions: 0, appRunning: false });
    if (url.pathname === '/api/logs/tail') return reply({ offset: 0, text: '', reset: false });
    if (url.pathname === '/api/updates') return reply({
      releases: [], checked_at: 0, auto_update: true, update_check_interval: 0,
      install_supported: true, update_available: true, phase: 'idle',
    });
    if (url.pathname === '/api/auth/sessions') return reply({ sessions: [] });
    return reply({ status: true, available: false, enabled: false });
  });
  return { page, writes };
}

async function nav(page, name) { await page.getByRole('navigation', { name: 'Main' }).getByRole('link', { name, exact: true }).click(); }
async function cancel(page) { await page.getByRole('dialog').getByRole('button', { name: 'Cancel', exact: true }).click(); }
async function unloadProtected(page) {
  assert.equal(await page.evaluate(() => !window.dispatchEvent(new Event('beforeunload', { cancelable: true }))), true);
}

test('settings drafts survive category changes and canceled navigation', async t => {
  const { page } = await fixture(t);
  await page.goto(base + '/logs');
  await nav(page, 'Settings');
  await page.getByLabel('Host name', { exact: true }).fill('Unsaved name');
  await page.getByRole('link', { name: /^Network/ }).click();
  await nav(page, 'Library');
  await cancel(page);
  assert.equal(new URL(page.url()).pathname, '/settings/network');
  await unloadProtected(page);
  await page.goBack();
  assert.equal(await page.getByLabel('Host name', { exact: true }).inputValue(), 'Unsaved name');
  await page.goBack();
  await cancel(page);
  await page.waitForURL(base + '/settings');
  assert.equal(await page.getByLabel('Host name', { exact: true }).inputValue(), 'Unsaved name');
  await page.goBack();
  await page.getByRole('dialog').getByRole('button', { name: 'Leave', exact: true }).click();
  await page.getByRole('heading', { name: 'Logs', exact: true }).waitFor();
  await page.goForward();
  assert.equal(await page.getByLabel('Host name', { exact: true }).inputValue(), 'Fixture host');
});

test('pairing a device leaves the app draft open and explains PIN verification', async t => {
  const { page, writes } = await fixture(t, { pending: [{ uniqueid: 'new-device', name: 'New device', age_seconds: 0 }] });
  await page.goto(base + '/logs');
  await nav(page, 'Library');
  await page.getByRole('link', { name: 'Add app', exact: true }).first().click();
  await page.getByLabel('Name', { exact: true }).fill('Unsaved app');
  await page.getByLabel('PIN for New device').fill('1234');
  await page.locator('.banner').getByRole('button', { name: 'Pair', exact: true }).click();
  await page.getByText(/If it rejects the PIN, start pairing again/).waitFor();
  assert(writes.includes('/api/pin'));
  assert.equal(new URL(page.url()).pathname, '/library/new');
  assert.equal(await page.getByLabel('Name', { exact: true }).inputValue(), 'Unsaved app');
  await unloadProtected(page);
  await page.goBack();
  await cancel(page);
  await page.waitForURL(base + '/library/new');
  assert.equal(await page.getByLabel('Name', { exact: true }).inputValue(), 'Unsaved app');
  await nav(page, 'Devices');
  await cancel(page);
  assert.equal(new URL(page.url()).pathname, '/library/new');
});

test('device drafts block links, tab closure and browser Back', async t => {
  const { page } = await fixture(t);
  await page.goto(base + '/logs');
  await nav(page, 'Devices');
  await page.getByRole('button', { name: 'Edit Test device', exact: true }).click();
  await page.getByLabel('Name', { exact: true }).fill('Unsaved device');
  await nav(page, 'Library');
  await cancel(page);
  await unloadProtected(page);
  await page.goBack();
  await cancel(page);
  await page.waitForURL(base + '/devices');
  assert.equal(await page.getByLabel('Name', { exact: true }).inputValue(), 'Unsaved device');
});

test('disabled pairing points to its setting instead of claiming no request is waiting', async t => {
  const { page } = await fixture(t, { pairing: false });
  await page.goto(base + '/devices');
  await page.getByText(/Pairing is disabled/).waitFor();
  assert.equal(await page.getByText('No device is waiting to pair.').count(), 0);
  await page.getByRole('link', { name: 'General settings', exact: true }).click();
  await page.getByRole('switch', { name: /^Allow pairing/ }).waitFor();
});

test('an unavailable host retries automatically and recovers with a usable navigation shell', async t => {
  const { page } = await fixture(t, { failures: 2 });
  await page.goto(base + '/settings/general');
  await page.getByRole('heading', { name: 'Cannot connect to Rubylight' }).waitFor();
  await page.getByRole('button', { name: 'Retry now' }).click();
  await page.getByRole('navigation', { name: 'Main' }).waitFor();
  assert.equal(new URL(page.url()).pathname, '/settings/general');
  await page.getByLabel('Fallback console language').waitFor();
  await page.getByText('This console is currently available in English only.', { exact: false }).waitFor();
});

test('Install now needs confirmation and disabled update checks disable automatic installation', async t => {
  const { page, writes } = await fixture(t);
  await page.goto(base + '/maintenance');
  await page.getByText('Off — automatic update checks are disabled', { exact: true }).waitFor();
  await page.getByRole('button', { name: 'Install now', exact: true }).click();
  await page.getByText(/Save your game progress before continuing/).waitFor();
  await cancel(page);
  assert(!writes.includes('/api/updates/install_now'));
  await page.getByRole('button', { name: 'Install now', exact: true }).click();
  if (process.env.BUTTERPOLLO_TEST_ARTIFACTS) {
    await page.screenshot({ path: path.join(process.env.BUTTERPOLLO_TEST_ARTIFACTS, 'console-install-confirmation.png') });
    await page.setViewportSize({ width: 390, height: 844 });
    assert(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
    await page.screenshot({ path: path.join(process.env.BUTTERPOLLO_TEST_ARTIFACTS, 'console-install-confirmation-mobile.png') });
  }
  const installed = page.waitForResponse(response => response.url().endsWith('/api/updates/install_now'));
  await page.getByRole('dialog').getByRole('button', { name: 'Install now', exact: true }).click();
  await installed;
  assert.equal(writes.filter(value => value === '/api/updates/install_now').length, 1);
});
