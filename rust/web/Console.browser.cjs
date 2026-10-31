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
  await warmUp(browser, base, ['/', '/devices', '/maintenance']);
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
    virtual_display: { capable: false, ready: false }, audio_enabled: false, features: {}, ...options.metadata,
  };
  page.on('pageerror', error => errors.push(error.message));
  t.after(async () => { await page.close(); assert.deepEqual(errors, []); });
  await page.route('**/*', async route => {
    const request = route.request(), url = new URL(request.url());
    assert.equal(url.origin, base, 'the fixture must not contact a real host');
    if (!url.pathname.startsWith('/api/')) return route.continue();
    const reply = body => route.fulfill({ json: body });
    // A test's own answers come first: { json } or { status, json }.
    const own = options.api?.(request.method(), url.pathname);
    if (own) {
      if (request.method() !== 'GET') writes.push(url.pathname);
      return route.fulfill({ status: own.status ?? 200, json: own.json });
    }
    if (request.method() !== 'GET') {
      writes.push(url.pathname);
      if (url.pathname === '/api/clients/update' && options.keepWrites) {
        const { uuid, ...changes } = request.postDataJSON();
        const device = options.clients.find(client => client.uuid === uuid);
        for (const [key, value] of Object.entries(changes)) { if (value === null) delete device[key]; else device[key] = value; }
      }
      if (url.pathname === '/api/auth/logout' && options.logoutFailure) {
        return route.fulfill({ status: 503, json: { error: 'Host temporarily unavailable' } });
      }
      if (url.pathname === '/api/pin') pending = [];
      if (url.pathname === '/api/pairing/qr') {
        return reply({ status: true, codes: [{ address: '127.0.0.1', svg: '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1 1"/>', uri: 'moonlight://pair?fixture' }], name: 'Fixture host', expires_in_seconds: 300 });
      }
      return reply({ status: true, uuid: 'saved-app' });
    }
    if (url.pathname === '/api/auth/status') {
      if (failures-- > 0) return route.fulfill({ status: 503, body: 'Host is restarting' });
      return reply({ status: true, credentials_configured: options.credentials !== false, authenticated: options.authenticated !== false });
    }
    if (url.pathname === '/api/csrf-token') return reply({ csrf_token: 'fixture-token' });
    if (url.pathname === '/api/config') return reply({ sunshine_name: 'Fixture host' });
    if (url.pathname === '/api/metadata') return options.metadataError ? route.fulfill({ status: 503, body: 'Host unavailable' }) : reply(metadata);
    if (url.pathname === '/api/clients/pending') return reply({ requests: pending });
    if (url.pathname === '/api/clients/list') return reply({ clients: options.clients ?? [{ uuid: 'client', name: 'Test device', enabled: true, perm: 0x071f1f00 }] });
    if (url.pathname === '/api/clients/hdr-profiles') return reply({ profiles: [] });
    if (url.pathname === '/api/display-devices') return reply([]);
    if (url.pathname === '/api/apps') return reply({ apps: options.apps ?? [] });
    if (url.pathname === '/api/session/status') return reply({ activeSessions: 0, appRunning: false });
    if (url.pathname === '/api/rtsp/sessions') return reply({ status: true, sessions: options.streams || [] });
    if (url.pathname === '/api/logs/tail') return reply({ offset: 0, text: '', reset: false });
    if (url.pathname === '/api/updates') return reply({
      releases: [], checked_at: 0, auto_update: true, update_check_interval: 0,
      install_supported: true, update_available: true, phase: 'idle', ...options.updates,
    });
    if (url.pathname === '/api/power') return reply({
      status: true,
      wake: { revision: 1, primary: null, targets: [],
        system: { sleep: 's3', modern_standby: false, hibernate: false, fast_startup: 'off', shutdown_wake: 'unknown' } },
      idle_sleep: { state: 'off', minutes: 0, reason: null, remaining_seconds: null }, last_sleep: null,
      fix: { running: false, changes: [], skipped: [], error: null, fast_startup_off_offered: false, last: null },
    });
    if (url.pathname === '/api/auth/sessions') return reply({ sessions: [] });
    if (url.pathname === '/api/tokens') return reply({ tokens: [] });
    if (url.pathname === '/api/usb') return reply({ status: true, devices: [] });
    if (url.pathname === '/api/token/routes') return reply({ routes: [] });
    // Help's panels, so every page renders without a page error.
    if (url.pathname === '/api/coop') return reply({ mode: { setting: 'nucleus', mode: 'nucleus', label: 'Nucleus Co-op', reason: '' },
      install: { state: 'missing', path: null, version: null, error: null }, available: true, reason: null, sessions: [] });
    if (url.pathname === '/api/logs/analysis') return reply({ status: true, findings: [], since: null, until: null, read_bytes: 0 });
    if (url.pathname === '/api/session/console') return reply({ status: true, recovery: { session_id: 1, console_session_id: 1, can_reattach: false, reason: '' } });
    if (url.pathname === '/api/webhooks') return reply({ status: true, events: ['app.started', 'app.stopped'], max: 8, hooks: options.hooks || [], error: null });
    return reply({ status: true, available: false, enabled: false });
  });
  return { page, writes, metadata };
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
  await page.locator('main').getByRole('link', { name: 'All settings', exact: true }).click();
  await page.getByLabel('Host name', { exact: true }).fill('Unsaved name');
  await page.getByRole('link', { name: /^Network/ }).click();
  await nav(page, 'Games');
  await cancel(page);
  assert.equal(new URL(page.url()).pathname, '/settings/network');
  await unloadProtected(page);
  await page.goBack();
  assert.equal(await page.getByLabel('Host name', { exact: true }).inputValue(), 'Unsaved name');
  await page.goBack();
  await cancel(page);
  await page.waitForURL(base + '/settings/general');
  assert.equal(await page.getByLabel('Host name', { exact: true }).inputValue(), 'Unsaved name');
  await page.goBack();
  await page.getByRole('dialog').getByRole('button', { name: 'Leave', exact: true }).click();
  await page.getByRole('heading', { name: 'Settings', exact: true, level: 1 }).waitFor();
  await page.goForward();
  assert.equal(await page.getByLabel('Host name', { exact: true }).inputValue(), 'Fixture host');
});

test('hardware metrics stay off until enabled and save the interval and selection', async t => {
  const { page } = await fixture(t);
  const consoleErrors = [];
  page.on('console', message => { if (['error', 'warning'].includes(message.type())) consoleErrors.push(message.text()); });
  await page.goto(base + '/settings/general');
  assert.equal(await page.title(), 'Rubylight');
  const enabled = page.getByRole('switch', { name: /^Send hardware metrics to client/ });
  await enabled.waitFor();
  assert.equal(await enabled.isChecked(), false);
  assert.equal(await page.getByLabel('Hardware metrics interval', { exact: true }).count(), 0);
  await page.getByText('Send hardware metrics to client', { exact: true }).click();
  const interval = page.getByLabel('Hardware metrics interval', { exact: true });
  assert.equal(await interval.inputValue(), '1000');
  await interval.fill('499');
  assert.equal(await interval.getAttribute('aria-invalid'), 'true');
  await page.getByText('Enter a number from 500 to 10000.', { exact: true }).waitFor();
  await interval.fill('750');
  const selection = page.getByRole('group', { name: 'Hardware metrics selection', exact: true });
  await selection.getByRole('button', { name: 'Add', exact: true }).click();
  await page.getByLabel('Hardware metrics selection, item 1', { exact: true }).fill('cpu_usage_percent');
  assert.equal(await page.locator('vite-error-overlay').count(), 0);
  if (process.env.BUTTERPOLLO_TEST_ARTIFACTS) {
    await enabled.scrollIntoViewIfNeeded();
    await page.screenshot({ path: path.join(process.env.BUTTERPOLLO_TEST_ARTIFACTS, 'hardware-metrics-desktop.png') });
    await page.setViewportSize({ width: 390, height: 844 });
    await selection.scrollIntoViewIfNeeded();
    assert(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
    await page.screenshot({ path: path.join(process.env.BUTTERPOLLO_TEST_ARTIFACTS, 'hardware-metrics-mobile.png') });
  }
  const saved = page.waitForRequest(request => new URL(request.url()).pathname === '/api/config' && request.method() !== 'GET');
  await page.getByRole('button', { name: 'Save changes', exact: true }).click();
  const body = (await saved).postDataJSON();
  assert.equal(body.hardware_metrics, true);
  assert.equal(body.hardware_metrics_interval, 750);
  assert.deepEqual(body.hardware_metrics_selection, ['cpu_usage_percent']);
  assert.deepEqual(consoleErrors, []);
});

test('pairing a device leaves the app draft open and explains PIN verification', async t => {
  const { page, writes } = await fixture(t, { pending: [{ uniqueid: 'new-device', name: 'New device', age_seconds: 0 }] });
  await page.goto(base + '/logs');
  await nav(page, 'Games');
  await page.getByRole('link', { name: 'Add manually', exact: true }).first().click();
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
  await page.getByRole('button', { name: 'Settings Test device', exact: true }).click();
  await page.getByLabel('Name', { exact: true }).fill('Unsaved device');
  await nav(page, 'Games');
  await cancel(page);
  await unloadProtected(page);
  await page.goBack();
  await cancel(page);
  await page.waitForURL(base + '/devices');
  assert.equal(await page.getByLabel('Name', { exact: true }).inputValue(), 'Unsaved device');
});

test('disabled pairing offers to turn it on right there instead of claiming no request is waiting', async t => {
  const { page } = await fixture(t, { pairing: false });
  await page.goto(base + '/devices');
  await page.getByText('Pairing is turned off', { exact: true }).waitFor();
  assert.equal(await page.getByText(/Waiting for a device/).count(), 0);
  await page.getByRole('button', { name: 'Allow pairing', exact: true }).waitFor();
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
  await page.goto(base + '/settings');
  await page.locator('summary', { hasText: 'about updates' }).click();
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

test('the next update check says when it runs, and that a stream holds it back', async t => {
  const now = Math.floor(Date.now() / 1000);
  const shot = async (page, name) => {
    if (process.env.BUTTERPOLLO_TEST_ARTIFACTS) await page.locator('section', { hasText: 'Next check' }).first().screenshot({ path: path.join(process.env.BUTTERPOLLO_TEST_ARTIFACTS, name) });
  };
  const scheduled = { checked_at: now - 600, update_check_interval: 86400, next_check_at: now + 85800, deferred: false };
  const normal = await fixture(t, { updates: scheduled });
  await normal.page.goto(base + '/settings');
  await normal.page.locator('summary', { hasText: 'about updates' }).click();
  await normal.page.getByText('Next check', { exact: true }).waitFor();
  assert.equal(await normal.page.getByText('After the stream').count(), 0);
  assert.equal(await normal.page.getByText('Shortly').count(), 0);
  await shot(normal.page, 'updates-panel-scheduled.png');
  const held = await fixture(t, { updates: { ...scheduled, checked_at: now - 90000, next_check_at: now - 3600, deferred: true } });
  await held.page.goto(base + '/settings');
  await held.page.locator('summary', { hasText: 'about updates' }).click();
  await held.page.getByText('After the stream', { exact: true }).waitFor();
  assert.equal(await held.page.getByText('Shortly').count(), 0);
  await shot(held.page, 'updates-panel-deferred.png');
  const overdue = await fixture(t, { updates: { ...scheduled, checked_at: now - 90000, next_check_at: now - 3600 } });
  await overdue.page.goto(base + '/settings');
  await overdue.page.locator('summary', { hasText: 'about updates' }).click();
  await overdue.page.getByText('Shortly', { exact: true }).waitFor();
});

test('signing out respects unsaved settings and only logs out after confirmation', async t => {
  const { page, writes } = await fixture(t);
  await page.goto(base + '/settings/general');
  await page.getByLabel('Host name', { exact: true }).fill('Keep this draft');
  await page.getByRole('button', { name: 'Sign out', exact: true }).click();
  await cancel(page);
  assert(!writes.includes('/api/auth/logout'));
  assert.equal(await page.getByLabel('Host name', { exact: true }).inputValue(), 'Keep this draft');
  await page.getByRole('button', { name: 'Sign out', exact: true }).click();
  await page.getByRole('dialog').getByRole('button', { name: 'Leave', exact: true }).click();
  await page.getByRole('heading', { name: 'Sign in', exact: true }).waitFor();
  assert.equal(writes.filter(value => value === '/api/auth/logout').length, 1);
});

test('a failed sign out retains the draft and allows a retry', async t => {
  const { page } = await fixture(t, { logoutFailure: true });
  await page.goto(base + '/settings/general');
  await page.getByLabel('Host name', { exact: true }).fill('Still editing');
  await page.getByRole('button', { name: 'Sign out', exact: true }).click();
  await page.getByRole('dialog').getByRole('button', { name: 'Leave', exact: true }).click();
  await page.getByText(/Signing out failed: Host temporarily unavailable/).waitFor();
  assert.equal(await page.getByLabel('Host name', { exact: true }).inputValue(), 'Still editing');
  assert.equal(await page.getByRole('button', { name: 'Sign out', exact: true }).isEnabled(), true);
});

test('small-screen navigation scrolls, supports Escape, and moves focus to the destination', async t => {
  const { page } = await fixture(t);
  await page.setViewportSize({ width: 640, height: 360 });
  await page.goto(base + '/logs');
  const menu = page.getByRole('button', { name: 'Menu', exact: true });
  await menu.click();
  const signOut = page.getByRole('button', { name: 'Sign out', exact: true });
  await signOut.scrollIntoViewIfNeeded();
  const bounds = await signOut.boundingBox();
  assert(bounds && bounds.y >= 0 && bounds.y + bounds.height <= 360);
  await page.keyboard.press('Escape');
  assert.equal(await menu.getAttribute('aria-expanded'), 'false');
  assert.equal(await menu.evaluate(element => element === document.activeElement), true);
  await menu.click();
  await nav(page, 'Settings');
  await page.getByLabel('Host name', { exact: true }).waitFor();
  assert.equal(await menu.getAttribute('aria-expanded'), 'false');
  assert.equal(await page.locator('main').evaluate(element => element === document.activeElement), true);
  assert(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
});

test('keyboard users can skip navigation and malformed URLs keep the console usable', async t => {
  const { page } = await fixture(t);
  await page.goto(base + '/logs');
  await page.getByRole('heading', { name: 'Logs', exact: true }).waitFor();
  await page.keyboard.press('Tab');
  assert.equal(await page.getByRole('link', { name: 'Skip to content', exact: true }).evaluate(element => element === document.activeElement), true);
  await page.keyboard.press('Enter');
  assert.equal(await page.locator('main').evaluate(element => element === document.activeElement), true);
  // Vite rejects malformed escapes before serving the app; exercise client routing directly.
  await page.evaluate(() => {
    history.pushState({ rubylightIndex: 1 }, '', '/settings/%broken');
    dispatchEvent(new PopStateEvent('popstate', { state: history.state }));
  });
  await page.getByRole('heading', { name: 'Page not found', exact: true }).waitFor();
  await nav(page, 'Settings');
  await page.getByLabel('Host name', { exact: true }).waitFor();
});

test('the sidebar groups tools, names each theme choice and copies the host address', async t => {
  const { page } = await fixture(t);
  await page.context().grantPermissions(['clipboard-read', 'clipboard-write'], { origin: base });
  await page.goto(base + '/');
  const sidebar = page.locator('aside');
  const main = page.getByRole('navigation', { name: 'Main' });
  // allInnerTexts does not wait: let the shell render its links first.
  await main.getByRole('link', { name: 'Help', exact: true }).waitFor();
  assert.deepEqual((await main.locator(':scope > .nav-group').getByRole('link').allInnerTexts()).map(text => text.trim()),
    ['Home', 'Games', 'Devices', 'Settings', 'Help']);
  // A desktop window keeps the whole sidebar in view with Advanced open: the links are
  // a calm 44-48 px apart and the footer (host, theme, sign out) is never pushed off.
  await page.setViewportSize({ width: 1440, height: 900 });
  await main.locator('summary', { hasText: 'Advanced' }).click();
  await main.getByRole('link', { name: 'Webhooks', exact: true }).waitFor();
  const pitch = await main.getByRole('link').evaluateAll(links => links.slice(1, 5).map((link, i) =>
    link.getBoundingClientRect().top - links[i].getBoundingClientRect().top));
  assert(pitch.every(value => value >= 44 && value <= 48), `nav pitch ${pitch}`);
  for (const height of [900, 600]) {
    await page.setViewportSize({ width: 1440, height });
    const signOut = await sidebar.getByRole('button', { name: 'Sign out', exact: true }).boundingBox();
    assert(signOut && signOut.y + signOut.height <= height, `Sign out ends at ${signOut?.y + signOut?.height}px in a ${height}px window`);
  }
  await page.setViewportSize({ width: 1280, height: 720 });
  await sidebar.getByText('Version 2.2.2', { exact: true }).waitFor();
  const dark = page.getByRole('group', { name: 'Theme', exact: true }).getByRole('button', { name: 'Dark theme', exact: true });
  assert.equal(await dark.getAttribute('title'), 'Theme: Dark');
  assert.equal(await dark.getAttribute('aria-pressed'), 'false');
  await dark.click();
  assert.equal(await dark.getAttribute('aria-pressed'), 'true');
  assert.equal(await page.locator('html').getAttribute('data-theme'), 'dark');
  await sidebar.getByRole('button', { name: 'Copy address', exact: true }).click();
  await page.getByText('Copied the address.', { exact: true }).waitFor();
  assert.equal(await page.evaluate(() => navigator.clipboard.readText()), '127.0.0.1');
  // The sidebar's background runs to the bottom of a long page.
  const heights = await page.evaluate(() => [document.querySelector('aside').getBoundingClientRect().height, document.documentElement.scrollHeight]);
  assert(heights[0] >= heights[1] - 1, `sidebar ${heights[0]}px on a ${heights[1]}px page`);
  // A short page still has the page colour down to the bottom of the window.
  await page.setViewportSize({ width: 390, height: 844 });
  await page.getByRole('button', { name: 'Menu', exact: true }).click();
  await nav(page, 'Settings');
  await page.getByRole('heading', { name: 'Settings', exact: true, level: 1 }).waitFor();
  await page.goto(base + '/tokens');
  await page.getByRole('heading', { name: 'API tokens', exact: true }).waitFor();
  const colours = await page.evaluate(() => [getComputedStyle(document.documentElement).backgroundColor, getComputedStyle(document.body).backgroundColor]);
  assert.notEqual(colours[0], 'rgba(0, 0, 0, 0)');
  assert.equal(colours[0], colours[1]);
});

test('notice links stay in the sentence, disclosures share one chevron, header actions sit on the title line', async t => {
  const { page } = await fixture(t);
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(base + '/devices');
  const summary = page.locator('summary', { hasText: 'Other ways to pair' });
  await summary.waitFor();
  assert.deepEqual(await summary.evaluate(element => [getComputedStyle(element).listStyleType, getComputedStyle(element, '::after').content]), ['none', '""']);
  // The shared .notice is a paragraph: a link inside its text stays one line of the
  // sentence, not a column of syllables ("Set/tin/gs" at 390 px).
  const height = await page.evaluate(() => {
    const notice = document.createElement('p');
    notice.className = 'notice';
    notice.innerHTML = 'Pairing is off. Turn on Allow pairing in <a href="/settings">Settings</a> to pair a new device. Devices already paired keep working.';
    document.querySelector('main').append(notice);
    const link = notice.querySelector('a').getBoundingClientRect().height;
    const line = parseFloat(getComputedStyle(notice).lineHeight);
    const display = getComputedStyle(notice).display;
    notice.remove();
    return { link, line, display };
  });
  assert.equal(height.display, 'block');
  assert(height.link < height.line * 1.5, `link ${height.link}px on ${height.line}px lines`);
  // Actions are centred on the h1 with the subtitle below, also without a subtitle.
  for (const [width, route] of [[1440, '/settings'], [1440, '/']]) {
    await page.setViewportSize({ width, height: 900 });
    await page.goto(base + route);
    const header = page.locator('.page-header');
    const [title, actions] = await Promise.all([header.locator('h1').boundingBox(), header.locator('.actions').boundingBox()]);
    assert(Math.abs(title.y + title.height / 2 - (actions.y + actions.height / 2)) <= 2, `${route} at ${width}px`);
  }
  // A phone puts them under the subtitle, or under the title when there is none.
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(base + '/settings');
  const [subtitle, actions] = await Promise.all([page.locator('.page-header .subtitle').boundingBox(), page.locator('.page-header .actions').boundingBox()]);
  assert(actions.y >= subtitle.y + subtitle.height);
  await page.goto(base + '/');
  const [title, homeActions] = await Promise.all([page.locator('.page-header h1').boundingBox(), page.locator('.page-header .actions').boundingBox()]);
  assert(homeActions.y >= title.y + title.height, 'Home actions under the title on a phone');
});

for (const [playnite, expected] of [
  [{ status: 503, json: { error: 'Playnite did not answer' } }, /Syncing Playnite failed: .*Playnite did not answer/],
  [{ json: { status: true, changed: true, game_count: 43 } }, /Your library is up to date with your launchers\./],
]) {
  test(`Sync now keeps the Playnite result: ${playnite.status ? 'a failure is shown' : 'a Playnite-only addition counts'}`, async t => {
    const { page } = await fixture(t, {
      apps: [{ uuid: 'desktop', name: 'Desktop' }],
      api: (method, path) => {
        if (path === '/api/playnite/status') return { json: { status: true, enabled: true, active: true, available: true, installed: true,
          installed_version: '1.4.0', packaged_version: '1.4.0', update_available: false, auto_sync: true, game_count: 42, synced_seconds_ago: 30 } };
        if (path === '/api/library/discovery') return { json: { status: true, sources: [{ source: 'steam', name: 'Steam', state: 'available', errors: [] }], games: [] } };
        if (method !== 'POST') return null;
        if (path === '/api/playnite/force_sync') return playnite;
        if (path === '/api/library/sync') return { json: { status: true, added: 0, removed: 0, playnite: true } };
        if (path === '/api/steam/force_sync') return { json: { status: true, changed: false, game_count: 4, importable_game_count: 0 } };
        return null;
      },
    });
    await page.goto(base + '/library');
    await page.getByText('Playnite', { exact: true }).first().waitFor();
    await page.getByRole('button', { name: 'Sync now' }).click();
    await page.getByText(expected).waitFor();
    if (playnite.status) await page.getByText('Nothing new: your library already has every game.').waitFor();
  });
}

test('on a phone a long game title in the editor keeps whole words above its actions', async t => {
  const apps = [{ uuid: 'mirage', name: 'Assassin’s Creed Mirage' }, { uuid: 'witcher', name: 'The Witcher 3: Wild Hunt' }, { uuid: 'desktop', name: 'Desktop' }];
  const { page } = await fixture(t, { apps });
  await page.setViewportSize({ width: 390, height: 844 });
  for (const app of apps.slice(0, 2)) {
    await page.goto(base + '/library/' + app.uuid);
    const title = page.getByRole('heading', { name: app.name, level: 1 });
    await title.waitFor();
    await page.getByRole('button', { name: 'Move later' }).waitFor();
    const fit = await title.evaluate(node => {
      const range = document.createRange();
      range.selectNodeContents(node);
      const lines = new Set([...range.getClientRects()].map(rect => Math.round(rect.top))).size;
      const words = node.textContent.split(' ').length;
      return { width: node.getBoundingClientRect().width, lines, words };
    });
    assert.ok(fit.width > 300, `${app.name}: title ${fit.width}px wide`);
    assert.ok(fit.lines < fit.words, `${app.name}: ${fit.lines} lines for ${fit.words} words`);
    const [box, actions] = await Promise.all([title.boundingBox(), page.locator('.page-header .actions').boundingBox()]);
    assert.ok(actions.y >= box.y + box.height, `${app.name}: actions under the title`);
  }
});

test('the first panel sits the same distance below the page header on every page', async t => {
  const { page } = await fixture(t);
  const gap = async (path, ready) => {
    await page.goto(base + path);
    await page.getByRole('heading', { name: ready, level: 1 }).waitFor();
    await page.waitForLoadState('networkidle');
    return page.locator('.page-header').first().evaluate(header => {
      let next = header.nextElementSibling;
      while (next && next.getBoundingClientRect().height === 0) next = next.nextElementSibling;
      return Math.round(next.getBoundingClientRect().top - header.getBoundingClientRect().bottom);
    });
  };
  const devices = await gap('/devices', 'Devices');
  // Help had an empty anchor that added a second grid gap above its first panel.
  assert.equal(await gap('/maintenance', 'Help'), devices);
});

test('on a phone, webhook rows line up with their panel title and the header name stays whole', async t => {
  const { page } = await fixture(t, { hooks: [{ url: 'https://home.example/hooks/rubylight', events: ['app.started'], last: null }] });
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(base + '/webhooks');
  const row = page.locator('li', { hasText: 'home.example' });
  await row.waitFor();
  const title = await page.getByRole('heading', { name: 'Webhooks', level: 2 }).evaluate(node => node.getBoundingClientRect().left);
  const content = await row.evaluate(node => node.getBoundingClientRect().left + parseFloat(getComputedStyle(node).paddingLeft));
  assert.ok(Math.abs(content - title) < 1, `row content at ${content}, title at ${title}`);
  const name = await page.locator('code', { hasText: 'X-Rubylight-Signature' }).evaluate(node =>
    [node.getClientRects().length, getComputedStyle(node).whiteSpace]);
  assert.deepEqual(name, [1, 'nowrap']);
});

test('normal PIN pairing stays primary while Artemis alternatives expand', async t => {
  const { page } = await fixture(t, { pending: [{ uniqueid: 'new-device', name: 'New device', age_seconds: 0 }] });
  await page.goto(base + '/devices');
  const pin = page.getByLabel('PIN shown on New device', { exact: true });
  await pin.waitFor();
  assert.equal(await page.getByLabel('Passphrase', { exact: true }).isVisible(), false);
  const other = page.locator('summary', { hasText: 'Other ways to pair' });
  await other.focus();
  await page.keyboard.press('Enter');
  await page.getByLabel('Passphrase', { exact: true }).waitFor();
  assert.equal(await pin.isVisible(), true);
  await other.press('Enter');
  assert.equal(await page.getByLabel('Passphrase', { exact: true }).isVisible(), false);
});

test('an empty library offers installed imports and manual additions', async t => {
  const { page } = await fixture(t);
  await page.goto(base + '/library');
  await page.getByRole('link', { name: 'Add games', exact: true }).first().waitFor();
  assert(await page.getByRole('link', { name: 'Add manually', exact: true }).count() >= 1);
  assert.equal(await page.getByRole('link', { name: 'Add games', exact: true }).first().getAttribute('href'), '/library/import');
});

test('a squeezed Browse button keeps its label on one line', async t => {
  const { page } = await fixture(t);
  await page.goto(base + '/library/new');
  const browse = page.getByRole('button', { name: 'Browse', exact: true }).first();
  await browse.waitFor();
  const box = await browse.boundingBox();
  assert(box.height >= 44 && box.height < 60, `Browse wrapped onto ${box.height}px`);
});

test('a device already waiting for a PIN shows as soon as the console loads', async t => {
  const { page } = await fixture(t, { pending: [{ uniqueid: 'waiting', name: 'Waiting phone', age_seconds: 5 }] });
  await page.goto(base + '/logs');
  await page.getByRole('navigation', { name: 'Main' }).waitFor();
  // The shell polls every 3 s; the request must not wait for that.
  await page.getByLabel('PIN for Waiting phone', { exact: true }).waitFor({ timeout: 2000 });
});

test('choosing a settings category from the keyboard keeps focus on that category', async t => {
  const { page } = await fixture(t);
  await page.goto(base + '/settings/general');
  const network = page.getByRole('link', { name: /^Network/ });
  await network.focus();
  await page.keyboard.press('Enter');
  await page.waitForURL(base + '/settings/network');
  await page.waitForTimeout(100);
  assert.equal(await network.evaluate(element => element === document.activeElement), true);
});

test('the stream card shows present to sent across the metrics, also on a phone', async t => {
  const spread = (mean_ms, p99_ms) => ({ mean_ms, p95_ms: p99_ms, p99_ms });
  const stream = {
    uuid: 'stream', device_name: 'Living room', role: 'stream', state: 'RUNNING', width: 2560, height: 1440,
    fps: 120, video_format: 1, hdr: false, vrr: false, encoder: 'amf', encoder_bitrate_kbps: 50_000,
    pyrowave_minimum_kbps: null, pyrowave_recommended_kbps: null, audio_channels: 2, frames_sent: 1200,
    bytes_sent: 0, idr_requests: 1, uptime_seconds: 10, warnings: [],
    performance: {
      fps: 120, bitrate_mbps: 30, encode_mean_ms: 3, encode_p95_ms: 3.3, encode_p99_ms: 3.5,
      host_processing_mean_ms: 3.29, host_processing_p95_ms: 3.6, host_processing_p99_ms: 3.99,
      host_processing_max_ms: 4.4, frame_age_mean_ms: 0.7, frame_age_p95_ms: 1.2, present_to_send_mean_ms: 4.3,
      send_interval_p99_ms: 9, sample_frames: 240, history: [],
      stages: { frames: 600, window_seconds: 5, present_to_sent: spread(4.26, 7.46), moonlight_host: spread(3.29, 3.99) },
    },
  };
  const { page } = await fixture(t, { streams: [stream] });
  await page.goto(base + '/');
  const card = page.getByRole('article', { name: 'Stream to Living room' });
  await card.locator('summary', { hasText: 'Stream details & performance' }).click();
  const label = card.getByText('Present to sent', { exact: true });
  await label.waitFor();
  const row = label.locator('xpath=..');
  assert.match(await row.innerText(), /4\.3 ms[\s\S]*Windows' present to the last packet sent[\s\S]*99% below 7\.5 ms/);
  // The end-to-end figure gets the whole width of the metrics, not a fifth column.
  const grid = await card.locator('dl.metrics').boundingBox();
  const cell = await row.boundingBox();
  const single = await card.getByText('Frame rate', { exact: true }).locator('xpath=..').boundingBox();
  // Within the metrics' 1px border.
  assert(cell.width >= grid.width - 3 && cell.width > single.width * 3, `row ${cell.width}px, metrics ${grid.width}px`);
  if (process.env.BUTTERPOLLO_TEST_ARTIFACTS) {
    await card.screenshot({ path: path.join(process.env.BUTTERPOLLO_TEST_ARTIFACTS, 'stream-card-desktop.png') });
  }
  await page.setViewportSize({ width: 390, height: 844 });
  await row.scrollIntoViewIfNeeded();
  assert(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
  const narrow = await row.boundingBox();
  assert(narrow.width <= 390);
  if (process.env.BUTTERPOLLO_TEST_ARTIFACTS) {
    await card.screenshot({ path: path.join(process.env.BUTTERPOLLO_TEST_ARTIFACTS, 'stream-card-mobile.png') });
  }
});


test('Home keeps loading, readiness, host errors and the library order distinct', async t => {
  const options = { metadata: { paired_devices: 0, encoder_status: { state: 'checking' } },
    apps: ['Fourth', 'First', 'Second', 'Third', 'Fifth'].map((name, index) => ({ name, uuid: String(index) })) };
  const { page, metadata } = await fixture(t, options);
  await page.clock.install();
  await page.goto(base + '/');
  await page.getByRole('heading', { name: 'Checking this PC…', exact: true }).waitFor();
  assert.equal(await page.getByRole('heading', { name: 'Ready to play', exact: true }).count(), 0);
  metadata.encoder_status = { state: 'ready', h264: true };
  metadata.capture_status = { virtual_display_configured: false, displays: [{ active: true, display_name: 'Monitor', width: 1920, height: 1080 }] };
  await page.clock.runFor(30_100);
  await page.getByRole('heading', { name: 'Let’s connect your device', exact: true }).waitFor();
  await page.getByRole('heading', { name: 'Get started', exact: true }).waitFor();
  assert.deepEqual(await page.locator('.game-shelf strong').allTextContents(), ['Fourth', 'First', 'Second', 'Third']);
  metadata.paired_devices = 1;
  await page.clock.runFor(30_100);
  await page.getByRole('heading', { name: 'Ready to play', exact: true }).waitFor();
  assert.equal(await page.getByRole('heading', { name: 'Get started', exact: true }).count(), 0);
  options.metadataError = true;
  await page.clock.runFor(30_100);
  await page.getByRole('heading', { name: 'Cannot check the host', exact: true }).waitFor();
  assert.equal(await page.getByRole('heading', { name: 'Ready to play', exact: true }).count(), 0);
  options.metadataError = false;
  metadata.encoder_status = { state: 'failed', h264: false };
  await page.getByRole('button', { name: 'Retry', exact: true }).click();
  await page.getByRole('heading', { name: 'This PC needs attention', exact: true }).waitFor();
  // The checks are already on the page: the hero's one View checks scrolls to them, and
  // the footer toggle offers the PC's details instead of a second View checks.
  const hero = page.getByRole('region', { name: 'Host status' });
  assert.equal(await hero.getByRole('button', { name: 'View checks', exact: true }).count(), 1);
  const details = hero.getByRole('button', { name: 'PC details', exact: true });
  assert.equal(await details.getAttribute('aria-expanded'), 'false');
  await hero.getByRole('button', { name: 'View checks', exact: true }).click();
  await page.getByRole('heading', { name: 'Needs attention', exact: true }).waitFor();
  assert.equal(await details.getAttribute('aria-expanded'), 'false', 'View checks must not toggle the details');
  await details.click();
  await page.getByRole('heading', { name: 'This PC', exact: true }).waitFor();
  await hero.getByRole('button', { name: 'Hide PC details', exact: true }).waitFor();
});

test('PIN submission waits for actual paired-device confirmation and keeps view-only permissions', async t => {
  const clients = [{ uuid: 'first', name: 'First device', enabled: true, connected: false, perm: 0x071f1f00 }];
  const { page } = await fixture(t, { clients, pending: [{ uniqueid: 'tablet', name: 'Tablet', age_seconds: 0 }] });
  await page.clock.install();
  await page.goto(base + '/devices');
  await page.locator('.device .name').filter({ hasText: 'First device' }).waitFor();
  const pin = page.getByLabel('PIN shown on Tablet', { exact: true });
  await pin.fill('12 34');
  assert.equal(await pin.inputValue(), '1234');
  await page.getByRole('button', { name: 'Pair device', exact: true }).click();
  await page.getByText(/^PIN sent to Tablet/).waitFor();
  assert.equal(await page.getByText('Tablet is paired.', { exact: true }).count(), 0);
  clients.push({ uuid: 'tablet', name: 'Tablet', enabled: true, connected: false, perm: 0x03000000, custom: 'keep', output_name_override: 'Other display' });
  await page.clock.runFor(5_100);
  await page.getByText('Tablet is paired.', { exact: true }).waitFor();
  await page.getByText('Can watch streams, but cannot start games or use controls.', { exact: true }).waitFor();
  await page.getByRole('button', { name: 'Change access Tablet', exact: true }).click();
  assert.equal(await page.getByLabel('Display', { exact: true }).isVisible(), false);
  await page.getByLabel('Name', { exact: true }).fill('Renamed tablet');
  const request = page.waitForRequest(request => new URL(request.url()).pathname === '/api/clients/update');
  await page.getByRole('button', { name: 'Save', exact: true }).click();
  const body = (await request).postDataJSON();
  assert.deepEqual(body, { uuid: 'tablet', name: 'Renamed tablet' });
  const saved = { ...clients[1], ...body };
  assert.equal(saved.perm, 0x03000000);
  assert.equal(saved.custom, 'keep');
  assert.equal(saved.output_name_override, 'Other display');
});


test('the host control checkbox drives the saved flag and keeps its section open', async t => {
  const clients = [{ uuid: 'phone', name: 'Phone', enabled: true, connected: true, perm: 0x071f1f00 }];
  const { page } = await fixture(t, { clients, keepWrites: true });
  await page.goto(base + '/devices');
  await page.getByRole('button', { name: 'Settings Phone', exact: true }).click();
  await page.getByText('Custom permissions', { exact: true }).click();
  const box = page.getByLabel('Host control: change Radeon settings and close games from this device', { exact: true });
  assert.equal(await box.isChecked(), false);
  const section = page.locator('details').filter({ has: page.locator('summary', { hasText: 'Custom permissions' }) });
  await box.check();
  // Host control isn't cut off by saving, so the stream warning stays away.
  await page.getByText('Unsaved changes', { exact: true }).waitFor();
  assert.equal(await page.getByText("Saving ends this device's current stream.").count(), 0);
  const request = page.waitForRequest(request => new URL(request.url()).pathname === '/api/clients/update');
  await page.getByRole('button', { name: 'Save', exact: true }).click();
  assert.deepEqual((await request).postDataJSON(), { uuid: 'phone', host_control: true });
  await page.getByText('Saved Phone.', { exact: true }).waitFor();
  assert.equal(await box.isChecked(), true);
  assert.equal(clients[0].host_control, true);
  await box.uncheck();
  assert.equal(await section.evaluate(details => details.open), true);
  // Removing a permission bit does end the stream, and the editor says so.
  await page.getByLabel('Keyboard', { exact: true }).uncheck();
  await page.getByText("Saving ends this device's current stream.").waitFor();
});


test('console journeys reflow across themes and screen sizes with usable buttons', async t => {
  const { page } = await fixture(t, { metadata: { capture_status: { displays: [{ display_name: 'Monitor' }] } } });
  for (const colorScheme of ['light', 'dark']) {
    await page.emulateMedia({ colorScheme, reducedMotion: 'reduce' });
    for (const viewport of [{ width: 1440, height: 900 }, { width: 834, height: 1112 }, { width: 390, height: 844 }, { width: 720, height: 450 }]) {
      await page.setViewportSize(viewport);
      for (const [route, heading] of [['/', 'Home'], ['/devices', 'Devices'], ['/settings/display', 'All settings'], ['/settings', 'Settings'], ['/library/new', 'New app'], ['/maintenance', 'Help']]) {
        await page.goto(base + route);
        await page.getByRole('heading', { name: heading, exact: true, level: 1 }).waitFor();
        assert(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), colorScheme + ' ' + viewport.width + ' ' + route);
        const small = await page.getByRole('button').evaluateAll(buttons => buttons.filter(button => {
          const rect = button.getBoundingClientRect();
          return !button.classList.contains('visually-hidden') && rect.width && rect.height && (rect.width < 43.5 || rect.height < 43.5);
        }).map(button => button.textContent.trim()));
        assert.deepEqual(small, [], 'Buttons need 44px targets on ' + route);
        if (process.env.BUTTERPOLLO_TEST_ARTIFACTS && viewport.width !== 720) {
          await page.screenshot({ path: path.join(process.env.BUTTERPOLLO_TEST_ARTIFACTS, (route.replaceAll('/', '-') || 'home') + '-' + colorScheme + '-' + viewport.width + '.png'), fullPage: true });
        }
      }
    }
  }
});

test('login and account setup remain readable on mobile in both themes', async t => {
  for (const credentials of [false, true]) {
    const { page } = await fixture(t, { credentials, authenticated: false });
    await page.setViewportSize({ width: 390, height: 844 });
    for (const colorScheme of ['light', 'dark']) {
      await page.emulateMedia({ colorScheme });
      await page.goto(base + '/');
      await page.getByRole('heading', { name: credentials ? 'Sign in' : 'Set up this host', exact: true }).waitFor();
      await page.getByLabel('Username', { exact: true }).fill('fixture-user');
      assert(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
      for (const input of await page.locator('input:not([type=checkbox])').all()) {
        assert((await input.boundingBox()).height >= 44);
      }
      if (credentials) assert((await page.locator('.remember').boundingBox()).height >= 44);
      if (process.env.BUTTERPOLLO_TEST_ARTIFACTS) await page.screenshot({ path: path.join(process.env.BUTTERPOLLO_TEST_ARTIFACTS, (credentials ? 'login' : 'setup') + '-' + colorScheme + '.png'), fullPage: true });
    }
  }
});

test('a WebRTC TV is badged on the PIN list and the Devices page, and keeps its normal controls', async t => {
  const clients = [
    { uuid: 'phone', name: 'Phone', enabled: true, connected: false, perm: 0x071f1f00 },
    { uuid: 'tv', name: 'Living room TV', enabled: false, connected: false, perm: 0x071f1f00, webrtc: { clientId: 'ab'.repeat(16) } },
  ];
  const { page } = await fixture(t, { clients, pending: [{ uniqueid: 'request', name: 'Den TV', age_seconds: 3, webrtc: true }] });
  await page.goto(base + '/devices');
  const tv = page.locator('.device').filter({ hasText: 'Living room TV' });
  await tv.getByText('WebRTC', { exact: true }).waitFor();
  await tv.getByText('Disabled', { exact: true }).waitFor();
  assert.equal(await page.locator('.device').filter({ hasText: 'Phone' }).getByText('WebRTC', { exact: true }).count(), 0);
  await page.getByRole('list', { name: 'Waiting for a PIN' }).getByText('WebRTC', { exact: true }).waitFor();
  if (process.env.BUTTERPOLLO_TEST_ARTIFACTS) {
    await page.screenshot({ path: path.join(process.env.BUTTERPOLLO_TEST_ARTIFACTS, 'b3-devices-webrtc-badge.png') });
  }
});
