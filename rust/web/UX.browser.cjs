// Optional Playwright runtime, with every API request isolated from a real host.
const assert = require('node:assert/strict');
const { before, after, test } = require('node:test');
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || 'playwright');
const path = require('node:path');

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

function deferred() {
  let resolve;
  const promise = new Promise(done => { resolve = done; });
  return { promise, resolve };
}

// `GET /api/power` as host/src/wake.rs console_status sends it.
function powerStatus(changes = {}) {
  return {
    status: true,
    wake: { revision: 1, primary: 'aa:bb:cc:dd:ee:01', targets: [{
      mac: 'aa:bb:cc:dd:ee:01', kind: 'ethernet', connected: true, route: true, broadcasts: ['192.168.4.255'], state: 'ready',
      checks: [{ id: 'wake_enabled', state: 'ok' }, { id: 'magic_packet', state: 'ok' }, { id: 'magic_packet_only', state: 'ok' }],
    }], system: { sleep: 's3', modern_standby: false, hibernate: false, fast_startup: 'off', shutdown_wake: 'unknown' }, ...changes.wake },
    idle_sleep: { state: 'off', minutes: 0, reason: null, remaining_seconds: null, ...changes.idle_sleep },
    last_sleep: null,
    fix: { running: false, changes: [], skipped: [], error: null, fast_startup_off_offered: false, last: null, ...changes.fix },
  };
}

async function fixture(t, intercept = async () => false) {
  const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
  page.setDefaultTimeout(10000);
  const errors = [], writes = [];
  page.on('pageerror', error => errors.push(error.message));
  t.after(async () => { await page.close(); assert.deepEqual(errors, []); });
  await page.route('**/*', async route => {
    const request = route.request(), url = new URL(request.url());
    assert.equal(url.origin, base, 'the fixture must not contact a real host');
    if (!url.pathname.startsWith('/api/')) return route.continue();
    if (request.method() !== 'GET') writes.push({ path: url.pathname, body: request.postDataJSON() });
    if (await intercept(route, url, request)) return;
    const reply = body => route.fulfill({ json: body });
    if (request.method() !== 'GET') return reply({ status: true });
    const replies = {
      '/api/auth/status': { status: true, credentials_configured: true, authenticated: true },
      '/api/csrf-token': { csrf_token: 'fixture-token' },
      '/api/config': { sunshine_name: 'Fixture host' },
      '/api/metadata': { version: '2.2.2', host_name: 'Fixture host', pc_address: '127.0.0.1',
        pc_addresses: [], paired_devices: 2, enable_pairing: false, warnings: [], features: {} },
      '/api/clients/pending': { requests: [] },
      '/api/clients/list': { clients: [
        { uuid: 'first', name: 'First device', enabled: true, connected: true, perm: 0x071f1f00 },
        { uuid: 'second', name: 'Second device', enabled: true, connected: false, perm: 0x071f1f00 },
      ] },
      '/api/clients/hdr-profiles': { profiles: [] },
      '/api/display-devices': [],
      '/api/apps': { apps: [{ uuid: 'one', name: 'One' }, { uuid: 'two', name: 'Two' }, { uuid: 'three', name: 'Three' }] },
      '/api/session/status': { activeSessions: 0, appRunning: false },
      '/api/updates': { releases: [], checked_at: 0, update_check_interval: 0,
        install_supported: true, update_available: false, phase: 'idle' },
      '/api/auth/sessions': { sessions: [] },
      '/api/display/golden_status': { status: true, exists: true, comparison_available: false, current_mismatch_reason: '' },
      '/api/frame-limiter/status': { enabled: false, configured_provider: 'none', active_provider: 'none', resolved_path: '' },
      '/api/power': powerStatus(),
    };
    return reply(replies[url.pathname] ?? { status: true, available: false, enabled: false });
  });
  return { page, writes };
}

test('maintenance panels recover failed loads without reloading the console', async t => {
  const failures = new Set(['/api/display/golden_status', '/api/health/crashdump', '/api/health/vulkan-hdr-layer',
    '/api/frame-limiter/status', '/api/auth/sessions', '/api/updates', '/api/power']);
  const { page } = await fixture(t, async (route, url) => {
    if (!failures.delete(url.pathname)) return false;
    await route.fulfill({ status: 503, body: 'Temporarily unavailable' });
    return true;
  });
  await page.goto(base + '/maintenance');
  for (const what of ['the display layout', 'crash reports', 'the Vulkan layer status',
    'the frame limiter status', 'signed-in browsers', 'the update status', 'the wake status']) {
    const retry = page.getByRole('button', { name: `Retry ${what}`, exact: true });
    await retry.click();
    await retry.waitFor({ state: 'detached' });
  }
  await page.getByText('A layout is saved. The host could not compare it with the current displays.', { exact: true }).waitFor();
  assert.equal(await page.getByText('A layout is saved and matches the displays as they are now.', { exact: true }).count(), 0);
  await page.setViewportSize({ width: 390, height: 844 });
  assert(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
});

test('host actions are disabled while the restart request is pending and recover after failure', async t => {
  const held = deferred(), started = deferred();
  t.after(() => held.resolve());
  const { page, writes } = await fixture(t, async (route, url) => {
    if (url.pathname !== '/api/restart') return false;
    started.resolve();
    await held.promise;
    await route.fulfill({ status: 503, body: 'Restart unavailable' });
    return true;
  });
  await page.goto(base + '/maintenance');
  const restart = page.getByRole('button', { name: 'Restart', exact: true });
  const quit = page.getByRole('button', { name: 'Quit', exact: true });
  await restart.click();
  await page.getByRole('dialog').getByRole('button', { name: 'Restart', exact: true }).click();
  await started.promise;
  assert.equal(await restart.isDisabled(), true);
  assert.equal(await quit.isDisabled(), true);
  assert.equal(await restart.getAttribute('aria-busy'), 'true');
  held.resolve();
  await page.getByText('Restart failed: Restart unavailable', { exact: true }).waitFor();
  assert.equal(await restart.isDisabled(), false);
  assert.equal(await quit.isDisabled(), false);
  assert.equal(writes.filter(write => write.path === '/api/restart').length, 1);
});

test('failed layout refresh keeps the last state visible and offers retry', async t => {
  let failNext = false;
  const { page } = await fixture(t, async (route, url) => {
    if (url.pathname === '/api/display/export_golden') failNext = true;
    if (url.pathname !== '/api/display/golden_status' || !failNext) return false;
    failNext = false;
    await route.fulfill({ status: 503, body: 'Layout temporarily unavailable' });
    return true;
  });
  await page.goto(base + '/maintenance');
  await page.getByRole('button', { name: 'Save current layout', exact: true }).click();
  await page.getByRole('dialog').getByRole('button', { name: 'Replace', exact: true }).click();
  await page.getByText(/Could not refresh the display layout.*Showing the last known state/).waitFor();
  assert.equal(await page.getByRole('button', { name: 'Restore saved layout', exact: true }).isVisible(), true);
  const retry = page.getByRole('button', { name: 'Retry the display layout', exact: true });
  await retry.click();
  await retry.waitFor({ state: 'detached' });
});

async function deviceActions(page) {
  await page.locator('details.more > summary').first().waitFor();
  for (const summary of await page.locator('details.more > summary').all()) await summary.click();
}

test('a device action locks other device actions until completion', async t => {
  const held = deferred(), started = deferred();
  t.after(() => held.resolve());
  const { page, writes } = await fixture(t, async (route, url) => {
    if (url.pathname !== '/api/clients/disconnect') return false;
    started.resolve();
    await held.promise;
    await route.fulfill({ json: { status: true } });
    return true;
  });
  await page.goto(base + '/devices');
  await deviceActions(page);
  await page.getByRole('button', { name: 'Disconnect First device', exact: true }).click();
  await page.getByRole('dialog').getByRole('button', { name: 'Disconnect', exact: true }).click();
  await started.promise;
  for (const name of ['Unpair all', 'Unpair First device', 'Settings First device', 'Unpair Second device', 'Settings Second device']) {
    assert.equal(await page.getByRole('button', { name, exact: true }).isDisabled(), true);
  }
  held.resolve();
  await page.getByText('Disconnected First device.', { exact: true }).waitFor();
  await page.getByRole('button', { name: 'Settings Second device', exact: true }).click();
  await page.getByRole('form', { name: 'Edit Second device', exact: true }).waitFor();
  assert.equal(writes.filter(write => write.path === '/api/clients/disconnect').length, 1);
});

test('a host that stops answering cannot keep the device actions locked', async t => {
  const held = deferred();
  t.after(() => held.resolve());
  let disconnected = false;
  const { page } = await fixture(t, async (route, url) => {
    if (url.pathname === '/api/clients/disconnect') disconnected = true;
    if (url.pathname !== '/api/clients/list' || !disconnected) return false;
    // The refresh after the action never gets an answer.
    await held.promise;
    await route.fulfill({ json: { status: true, clients: [] } }).catch(() => {});
    return true;
  });
  await page.clock.install();
  await page.goto(base + '/devices');
  await deviceActions(page);
  await page.getByRole('button', { name: 'Disconnect First device', exact: true }).click();
  await page.getByRole('dialog').getByRole('button', { name: 'Disconnect', exact: true }).click();
  await page.getByText('Disconnected First device.', { exact: true }).waitFor();
  const unpair = page.getByRole('button', { name: 'Unpair Second device', exact: true });
  assert.equal(await unpair.isDisabled(), true);
  await page.clock.runFor(21_000);
  await page.getByText(/Could not refresh the list: The host did not answer within 20 seconds/).waitFor();
  assert.equal(await unpair.isDisabled(), false);
  assert.equal(await page.getByRole('button', { name: 'Settings Second device', exact: true }).isDisabled(), false);
});

test('the global close-on-disconnect default saves and survives a reload', async t => {
  const config = {};
  const { page, writes } = await fixture(t, async (route, url, request) => {
    if (url.pathname !== '/api/config') return false;
    if (request.method() === 'GET') await route.fulfill({ json: config });
    else {
      Object.assign(config, request.postDataJSON());
      await route.fulfill({ json: { status: true } });
    }
    return true;
  });
  const messages = [];
  page.on('console', message => { if (['error', 'warning'].includes(message.type())) messages.push(message.text()); });
  t.after(() => assert.deepEqual(messages, []));
  const label = 'Close the app when the last device disconnects';
  await page.goto(base + '/settings/library');
  const toggle = page.getByRole('switch', { name: label, exact: true });
  await toggle.waitFor();
  assert.equal(await toggle.isChecked(), false);
  await toggle.check();
  await page.getByRole('button', { name: 'Save changes', exact: true }).click();
  await page.getByText('Saved. Some changes apply after the host restarts.', { exact: true }).waitFor();
  assert.equal(writes.at(-1).body.terminate_on_pause, true);
  await page.reload();
  await toggle.waitFor();
  assert.equal(await toggle.isChecked(), true);
  assert.equal(await page.title(), 'Rubylight');
  assert.equal(page.url(), base + '/settings/library');
  if (process.env.BUTTERPOLLO_TEST_ARTIFACTS) {
    await page.screenshot({ path: path.join(process.env.BUTTERPOLLO_TEST_ARTIFACTS, 'disconnect-global-desktop.png') });
  }
  await page.setViewportSize({ width: 390, height: 844 });
  assert(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
  if (process.env.BUTTERPOLLO_TEST_ARTIFACTS) {
    await page.screenshot({ path: path.join(process.env.BUTTERPOLLO_TEST_ARTIFACTS, 'disconnect-global-mobile.png') });
  }
});

test('app disconnect overrides preserve explicit Off and can return to inheritance', async t => {
  let app = { uuid: 'one', name: 'One', 'migration-field': { preserve: true } };
  const { page, writes } = await fixture(t, async (route, url, request) => {
    if (url.pathname !== '/api/apps') return false;
    if (request.method() === 'GET') await route.fulfill({ json: { apps: [app] } });
    else {
      app = request.postDataJSON();
      await route.fulfill({ json: { status: true, uuid: app.uuid } });
    }
    return true;
  });
  const messages = [];
  page.on('console', message => { if (['error', 'warning'].includes(message.type())) messages.push(message.text()); });
  t.after(() => assert.deepEqual(messages, []));
  await page.goto(base + '/library/one');
  await page.locator('summary', { hasText: /^Behavior/ }).click();
  const select = page.getByLabel('Close the app when the last device disconnects', { exact: true });
  await select.waitFor();
  assert.equal(await select.inputValue(), '');
  await select.scrollIntoViewIfNeeded();
  if (process.env.BUTTERPOLLO_TEST_ARTIFACTS) {
    await page.screenshot({ path: path.join(process.env.BUTTERPOLLO_TEST_ARTIFACTS, 'disconnect-app-desktop.png') });
  }
  for (const [value, expected] of [['off', false], ['on', true], ['', undefined]]) {
    await select.selectOption(value);
    const response = page.waitForResponse(response => response.url().endsWith('/api/apps') && response.request().method() === 'POST');
    await page.getByRole('button', { name: 'Save', exact: true }).click();
    await response;
    assert.equal(app['terminate-on-pause'], expected);
    assert.deepEqual(app['migration-field'], { preserve: true });
    if (expected === undefined) assert.equal(Object.hasOwn(writes.at(-1).body, 'terminate-on-pause'), false);
    await page.reload();
    await page.locator('summary', { hasText: /^Behavior/ }).click();
    await select.waitFor();
    assert.equal(await select.inputValue(), value);
  }
  for (const [stored, selected] of [['yes', 'on'], ['1', 'on'], ['false', 'off'], ['0', 'off']]) {
    app['terminate-on-pause'] = stored;
    await page.reload();
    await page.locator('summary', { hasText: /^Behavior/ }).click();
    await select.waitFor();
    assert.equal(await select.inputValue(), selected);
  }
  await page.setViewportSize({ width: 390, height: 844 });
  await select.scrollIntoViewIfNeeded();
  assert(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
  if (process.env.BUTTERPOLLO_TEST_ARTIFACTS) {
    await page.screenshot({ path: path.join(process.env.BUTTERPOLLO_TEST_ARTIFACTS, 'disconnect-app-mobile.png') });
  }
});

test('an app launch that waits on preparation commands is not cut short by the request timeout', async t => {
  const held = deferred(), started = deferred();
  t.after(() => held.resolve());
  const { page } = await fixture(t, async (route, url) => {
    if (url.pathname !== '/api/apps/launch') return false;
    started.resolve();
    await held.promise;
    await route.fulfill({ json: { status: true } });
    return true;
  });
  await page.clock.install();
  await page.goto(base + '/library/one');
  await page.getByRole('button', { name: 'Launch on PC', exact: true }).click();
  await started.promise;
  // Each preparation command may run for two minutes.
  await page.clock.runFor(3 * 60_000);
  held.resolve();
  await page.getByText('Started One.', { exact: true }).waitFor();
});

test('saving a device keeps its editor open and blocks conflicting actions', async t => {
  const held = deferred(), started = deferred();
  t.after(() => held.resolve());
  const { page } = await fixture(t, async (route, url) => {
    if (url.pathname !== '/api/clients/update') return false;
    started.resolve();
    await held.promise;
    await route.fulfill({ status: 503, body: 'Device save unavailable' });
    return true;
  });
  await page.goto(base + '/devices');
  await deviceActions(page);
  await page.getByRole('button', { name: 'Settings First device', exact: true }).click();
  await page.getByLabel('Name', { exact: true }).fill('New name');
  await page.getByRole('button', { name: 'Save', exact: true }).click();
  await started.promise;
  for (const name of ['Unpair all', 'Unpair First device', 'Close First device', 'Settings Second device']) {
    assert.equal(await page.getByRole('button', { name, exact: true }).isDisabled(), true);
  }
  held.resolve();
  await page.getByText('Saving the device failed: Device save unavailable', { exact: true }).waitFor();
  assert.equal(await page.getByLabel('Name', { exact: true }).inputValue(), 'New name');
  assert.equal(await page.getByRole('button', { name: 'Save', exact: true }).isDisabled(), false);
});

test('library reordering cannot overlap writes and restores the server order after failure', async t => {
  const held = deferred(), started = deferred();
  t.after(() => held.resolve());
  const { page, writes } = await fixture(t, async (route, url) => {
    if (url.pathname !== '/api/apps/reorder') return false;
    started.resolve();
    await held.promise;
    await route.fulfill({ status: 503, body: 'Order not saved' });
    return true;
  });
  await page.goto(base + '/library');
  await page.getByRole('link', { name: 'One', exact: true }).waitFor();
  await page.locator('[data-uuid="one"]').evaluate(node => {
    const target = document.querySelector('[data-uuid="two"]');
    const dataTransfer = new DataTransfer();
    node.dispatchEvent(new DragEvent('dragstart', { bubbles: true, cancelable: true, dataTransfer }));
    target.dispatchEvent(new DragEvent('dragover', { bubbles: true, cancelable: true, dataTransfer }));
    target.dispatchEvent(new DragEvent('drop', { bubbles: true, cancelable: true, dataTransfer }));
  });
  await started.promise;
  await page.getByText('Saving the library order…', { exact: true }).waitFor();
  assert.equal(await page.locator('[data-uuid][draggable="true"]').count(), 0);
  assert.deepEqual(writes.find(write => write.path === '/api/apps/reorder').body.order, ['two', 'one', 'three']);
  await page.getByRole('link', { name: 'One', exact: true }).click();
  await page.getByText('Saving the library order…', { exact: true }).waitFor();
  assert.equal(await page.getByRole('button', { name: 'Move earlier', exact: true }).isDisabled(), true);
  assert.equal(await page.getByRole('button', { name: 'Move later', exact: true }).isDisabled(), true);
  await page.getByRole('navigation', { name: 'Main' }).getByRole('link', { name: 'Games & apps', exact: true }).click();
  await page.getByText('Saving the library order…', { exact: true }).waitFor();
  held.resolve();
  await page.getByText('Saving the order failed: Order not saved', { exact: true }).waitFor();
  await page.locator('[data-uuid="one"][draggable="true"]').waitFor();
  assert.deepEqual(await page.locator('[data-uuid]').evaluateAll(nodes => nodes.map(node => node.dataset.uuid)), ['one', 'two', 'three']);
  assert.equal(writes.filter(write => write.path === '/api/apps/reorder').length, 1);
});

test('Unpair all only appears when there is a device to unpair', async t => {
  const { page } = await fixture(t, async (route, url) => {
    if (url.pathname !== '/api/clients/list') return false;
    await route.fulfill({ json: { status: true, clients: [] } });
    return true;
  });
  await page.goto(base + '/devices');
  await page.getByText('No paired devices', { exact: true }).waitFor();
  assert.equal(await page.getByRole('button', { name: 'Unpair all' }).count(), 0);
  assert.equal(await page.getByText('0 devices', { exact: true }).count(), 0);
});

test('a PIN typed for one waiting device is never sent to the next one', async t => {
  let pending = [{ uniqueid: 'a', name: 'Phone A', age_seconds: 1 }];
  const { page, writes } = await fixture(t, async (route, url) => {
    if (url.pathname !== '/api/clients/pending') return false;
    await route.fulfill({ json: { status: true, requests: pending } });
    return true;
  });
  await page.goto(base + '/library');
  const banner = page.locator('.banner');
  const first = page.getByLabel('PIN for Phone A', { exact: true });
  await first.fill('12 34');
  assert.equal(await first.inputValue(), '1234');
  pending = [{ uniqueid: 'b', name: 'Phone B', age_seconds: 1 }];
  const second = page.getByLabel('PIN for Phone B', { exact: true });
  await second.waitFor();
  assert.equal(await second.inputValue(), '');
  assert.equal(await banner.getByRole('button', { name: 'Pair', exact: true }).isDisabled(), true);
  await second.fill('5678');
  await banner.getByRole('button', { name: 'Pair', exact: true }).click();
  await page.getByText(/^PIN sent to Phone B/).waitFor();
  assert.deepEqual(writes.filter(write => write.path === '/api/pin').map(write => write.body), [{ pin: '5678', uniqueid: 'b', name: 'Phone B' }]);
});

test('a rejected PIN removes the expired request at once instead of on the next poll', async t => {
  let pending = [{ uniqueid: 'gone', name: 'Old phone', age_seconds: 299 }];
  const { page } = await fixture(t, async (route, url) => {
    if (url.pathname === '/api/metadata') {
      await route.fulfill({ json: { version: '2.2.2', host_name: 'Fixture host', pc_address: '127.0.0.1', pc_addresses: [],
        paired_devices: 2, enable_pairing: true, warnings: [], features: {} } });
      return true;
    }
    if (url.pathname === '/api/clients/pending') {
      await route.fulfill({ json: { status: true, requests: pending } });
      return true;
    }
    if (url.pathname !== '/api/pin') return false;
    pending = [];
    await route.fulfill({ status: 400, json: { error: 'pairing request no longer exists' } });
    return true;
  });
  await page.clock.install();
  await page.goto(base + '/devices');
  const pin = page.getByLabel('PIN shown on Old phone', { exact: true });
  await pin.fill('1234');
  // No more polling from here on: only the failure itself may refresh the list.
  await page.clock.pauseAt(new Date(Date.now() + 60_000));
  await page.getByRole('button', { name: 'Pair device', exact: true }).click();
  await page.getByText('Pairing failed: pairing request no longer exists', { exact: true }).waitFor();
  await pin.waitFor({ state: 'detached' });
});

test('maintenance is grouped into sections and every destructive action looks and confirms the same', async t => {
  const { page, writes } = await fixture(t);
  await page.goto(base + '/maintenance');
  await page.getByRole('button', { name: 'Restore saved layout', exact: true }).waitFor();
  assert.deepEqual(await page.locator('main').getByRole('heading', { level: 2 }).allInnerTexts(),
    ['Host', 'Power', 'Displays and Windows session', 'Troubleshooting', 'Integrations', 'Security']);
  for (const [name, title, confirmName] of [
    ['Quit', 'Quit Rubylight?', 'Quit'],
    ['Delete saved layout', 'Delete the saved layout?', 'Delete'],
    ['Disconnect', 'Disconnect virtual displays?', 'Disconnect'],
    ['Reset', 'Reset display settings memory?', 'Reset'],
  ]) {
    const button = page.locator('main').getByRole('button', { name, exact: true });
    assert.match(await button.getAttribute('class'), /\bdanger\b/, `${name} looks destructive`);
    await button.click();
    const dialog = page.getByRole('dialog');
    await dialog.getByRole('heading', { name: title, exact: true }).waitFor();
    assert.match(await dialog.getByRole('button', { name: confirmName, exact: true }).getAttribute('class'), /\bdanger\b/);
    await dialog.getByRole('button', { name: 'Cancel', exact: true }).click();
    await dialog.waitFor({ state: 'hidden' });
  }
  assert.deepEqual(writes, []);
});

test('Wake-on-LAN changes apply only after confirmation, exactly as listed, and a stale list is checked again', async t => {
  const first = ['Realtek PCIe GbE: allow this device to wake the computer'];
  const now = [...first, 'Realtek PCIe GbE: turn on Wake on magic packet'];
  const needs = { wake: { targets: [{ mac: 'aa:bb:cc:dd:ee:01', kind: 'ethernet', connected: true, route: true, broadcasts: [],
    state: 'needs_setup', checks: [{ id: 'wake_enabled', state: 'error' }, { id: 'magic_packet', state: 'ok' }] }],
    system: { sleep: 's3', modern_standby: false, hibernate: true, fast_startup: 'active', shutdown_wake: 'firmware_unknown' } },
    idle_sleep: { state: 'armed', minutes: 30, remaining_seconds: 720, reason: 'waiting for console activity' } };
  let offered = first, posts = 0, polls = 0, applied = false;
  const { page, writes } = await fixture(t, async (route, url, request) => {
    if (url.pathname === '/api/power' && request.method() === 'GET') {
      // The fix runs for one poll after the 202, then its result is there.
      const running = applied && polls++ === 0;
      const last = applied && !running ? { at: Math.floor(Date.now() / 1000), changes: now, restarted: ['Realtek PCIe GbE'], after_reboot: [], errors: [] } : null;
      await route.fulfill({ json: powerStatus({ ...needs, fix: { changes: offered, fast_startup_off_offered: true, running, last } }) });
      return true;
    }
    if (url.pathname !== '/api/power/wake/fix') return false;
    // The adapters changed after the first check: the host refuses and lists the changes now.
    if (++posts === 1) {
      offered = now;
      await route.fulfill({ status: 409, json: { status: false, error: "not applied: the adapters' settings changed since you checked; check again", changes: now, skipped: [] } });
    } else {
      applied = true;
      await route.fulfill({ status: 202, json: { status: true, changes: [...now, 'Turn off Fast Startup'], skipped: [] } });
    }
    return true;
  });
  await page.goto(base + '/maintenance');
  const panel = page.locator('section.panel', { has: page.getByRole('heading', { name: 'Wake-on-LAN and sleep', exact: true }) });
  await panel.getByText(/A wake setting of the wired adapter aa:bb:cc:dd:ee:01 is off/).waitFor();
  await panel.getByText('Sleeps in 12 min if nothing changes (counted from the last use of this console).', { exact: true }).waitFor();
  await panel.getByText(/Fast Startup is on: Shut down hibernates the PC/).waitFor();
  const apply = panel.getByRole('button', { name: 'Apply changes', exact: true });
  await apply.click();
  await page.getByRole('dialog').getByRole('button', { name: 'Cancel', exact: true }).click();
  assert.deepEqual(writes, []);
  await apply.click();
  await page.getByRole('dialog').getByRole('button', { name: 'Apply changes', exact: true }).click();
  await page.getByText(/Wake-on-LAN changes not applied: not applied: the adapters' settings changed since you checked/).waitFor();
  await panel.getByText(now[1], { exact: true }).waitFor();
  await panel.getByText('Also turn off Fast Startup', { exact: true }).click();
  await apply.click();
  await page.getByRole('dialog').getByRole('button', { name: 'Apply changes', exact: true }).click();
  await page.getByText('Wake-on-LAN changes applied.', { exact: true }).waitFor();
  await panel.getByText(/Applied 2 changes just now\.\s+Restarted: Realtek PCIe GbE\./).waitFor();
  // What the administrator saw, the first time and after checking again.
  assert.deepEqual(writes.map(write => write.body), [
    { changes: first, fast_startup_off: false },
    { changes: now, fast_startup_off: true },
  ]);
  if (process.env.BUTTERPOLLO_TEST_ARTIFACTS) await panel.screenshot({ path: path.join(process.env.BUTTERPOLLO_TEST_ARTIFACTS, 'power-panel.png') });
  await page.setViewportSize({ width: 390, height: 844 });
  assert(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
});

test('refreshing the Windows session status spins only its own button', async t => {
  const held = deferred();
  t.after(() => held.resolve());
  let calls = 0;
  const { page } = await fixture(t, async (route, url) => {
    if (url.pathname !== '/api/session/console') return false;
    if (++calls === 2) await held.promise;
    await route.fulfill({ json: { status: true, recovery: { session_id: 2, console_session_id: 1, can_reattach: true,
      reason: 'The desktop is in a Remote Desktop session.' } } });
    return true;
  });
  await page.goto(base + '/maintenance');
  await page.getByText('The desktop is in a Remote Desktop session.', { exact: true }).waitFor();
  const refresh = page.getByRole('button', { name: 'Refresh status', exact: true });
  const back = page.getByRole('button', { name: 'Return to console', exact: true });
  await refresh.click();
  assert.equal(await refresh.getAttribute('aria-busy'), 'true');
  assert.equal(await back.getAttribute('aria-busy'), 'false');
  assert.equal(await back.isDisabled(), true);
  held.resolve();
  await page.waitForFunction(() => [...document.querySelectorAll('button')].find(button => button.textContent.includes('Refresh status'))?.getAttribute('aria-busy') === 'false');
  assert.equal(await back.isDisabled(), false);
});

test('an update action spins only its own button, and update settings open inside the console', async t => {
  const held = deferred(), started = deferred();
  t.after(() => held.resolve());
  const { page } = await fixture(t, async (route, url, request) => {
    if (url.pathname === '/api/updates' && request.method() === 'GET') {
      await route.fulfill({ json: { releases: [{ tag_name: 'v2.2.3', html_url: 'https://example.invalid/release', body: '' }],
        latest_version: 'v2.2.3', checked_at: 1, update_check_interval: 6, auto_update: false,
        install_supported: true, update_available: true, phase: 'idle' } });
      return true;
    }
    if (url.pathname !== '/api/updates/install') return false;
    started.resolve();
    await held.promise;
    await route.fulfill({ json: { status: true } });
    return true;
  });
  await page.goto(base + '/maintenance');
  const install = page.getByRole('button', { name: 'Install when idle', exact: true });
  await install.click();
  await started.promise;
  assert.equal(await install.getAttribute('aria-busy'), 'true');
  assert.equal(await page.getByRole('button', { name: 'Check now', exact: true }).getAttribute('aria-busy'), 'false');
  held.resolve();
  await page.waitForFunction(() => [...document.querySelectorAll('button')].every(button => button.getAttribute('aria-busy') !== 'true'));
  await page.evaluate(() => { window.consoleStayedLoaded = true; });
  await page.getByRole('link', { name: 'Change update settings', exact: true }).click();
  await page.waitForURL(base + '/settings/general');
  assert.equal(await page.evaluate(() => window.consoleStayedLoaded), true);
});

test('an update that setup refused shows setup\'s reason once', async t => {
  const reason = 'Rubylight cannot be updated in D:\\Games\\Rubylight: D:\\Games must be owned by Administrators, SYSTEM or TrustedInstaller. '
    + 'Run the Rubylight installer from the release page: it moves Rubylight to C:\\Program Files\\Rubylight and keeps its settings and paired devices.';
  let state = { releases: [], checked_at: 1, update_check_interval: 0, install_supported: true, update_available: false,
    phase: 'failed', error: reason, last_install: { version: '2.2.22', phase: 'failed', error: reason } };
  const { page } = await fixture(t, async (route, url, request) => {
    if (url.pathname !== '/api/updates' || request.method() !== 'GET') return false;
    await route.fulfill({ json: state });
    return true;
  });
  await page.goto(base + '/maintenance');
  const panel = page.locator('section', { has: page.getByRole('heading', { name: 'Updates', exact: true }) });
  const shown = panel.getByText(reason, { exact: true });
  await shown.waitFor();
  assert.equal(await shown.count(), 1);
  assert.equal(await shown.getAttribute('role'), 'alert');
  assert.equal(await panel.getByText(/did not report completion/).count(), 0);
  // After the host restarts, the record alone says it.
  state = { ...state, phase: 'idle', error: null };
  await page.evaluate(() => document.dispatchEvent(new Event('visibilitychange')));
  await panel.getByRole('status').filter({ hasText: reason }).waitFor();
  assert.equal(await shown.count(), 1);
  assert.equal(await panel.getByRole('alert').count(), 0);
});

test('API tokens say when a refresh fails, and both lists can be retried', async t => {
  let listCalls = 0, routeCalls = 0;
  const { page } = await fixture(t, async (route, url, request) => {
    if (url.pathname === '/api/tokens' && request.method() === 'GET') {
      if (++listCalls === 2) {
        await route.fulfill({ status: 503, body: 'Tokens unavailable' });
        return true;
      }
      const tokens = listCalls === 1 ? [{ hash: 'h1', created_at: 1, scopes: [{ path: '/api/apps', methods: ['GET'] }] }] : [];
      await route.fulfill({ json: { status: true, tokens } });
      return true;
    }
    if (url.pathname === '/api/token/routes') {
      if (++routeCalls === 1) {
        await route.fulfill({ status: 503, body: 'Routes unavailable' });
        return true;
      }
      await route.fulfill({ json: { status: true, routes: [{ path: '/api/apps', methods: ['GET'] }] } });
      return true;
    }
    return false;
  });
  await page.goto(base + '/tokens');
  await page.getByText('Could not load the API paths: Routes unavailable', { exact: true }).waitFor();
  await page.getByRole('button', { name: 'Retry the API paths', exact: true }).click();
  await page.getByRole('checkbox', { name: 'GET /api/apps', exact: true }).waitFor();
  await page.getByRole('button', { name: 'Revoke', exact: true }).click();
  await page.getByRole('dialog').getByRole('button', { name: 'Revoke', exact: true }).click();
  await page.getByText('Could not refresh the tokens: Tokens unavailable. Showing the last known state.', { exact: true }).waitFor();
  await page.getByRole('button', { name: 'Retry the tokens', exact: true }).click();
  await page.getByText('No API tokens', { exact: true }).waitFor();
});

test('API tokens can select every offered permission beyond 64 paths, clear and customize that selection', async t => {
  const routes = Array.from({ length: 80 }, (_, index) => ({ path: `/api/fixture/${index}`, methods: ['GET', 'PATCH'] }));
  const { page, writes } = await fixture(t, async (route, url, request) => {
    if (url.pathname === '/api/tokens') {
      await route.fulfill({ json: { status: true, tokens: [] } });
      return true;
    }
    if (url.pathname === '/api/token/routes') {
      await route.fulfill({ json: { status: true, routes } });
      return true;
    }
    if (url.pathname === '/api/token' && request.method() === 'POST') {
      assert.equal(request.headers()['x-csrf-token'], 'fixture-token');
      await route.fulfill({ json: { status: true, token: 'fixture-created-token' } });
      return true;
    }
    return false;
  });
  const consoleErrors = [];
  page.on('console', message => { if (['error', 'warning'].includes(message.type())) consoleErrors.push(message.text()); });
  await page.goto(base + '/tokens');
  const all = page.getByRole('button', { name: 'Select all permissions', exact: true });
  const clear = page.getByRole('button', { name: 'Clear selection', exact: true });
  const create = page.getByRole('button', { name: 'Create token', exact: true });
  await all.waitFor();
  assert.equal(await create.isDisabled(), true);
  await page.getByRole('searchbox', { name: 'Search API paths' }).fill('/api/fixture/79');
  assert.equal(await page.getByRole('checkbox').count(), 2);
  await all.click();
  await page.getByText('160 permissions on 80 paths', { exact: true }).waitFor();
  assert.equal(await create.isEnabled(), true);
  assert.equal(await all.isDisabled(), true);
  await clear.click();
  assert.equal(await create.isDisabled(), true);
  assert.equal(await page.getByRole('checkbox', { checked: true }).count(), 0);
  await all.click();
  await page.getByRole('checkbox', { name: 'PATCH /api/fixture/79', exact: true }).uncheck();
  await page.getByText('159 permissions on 80 paths', { exact: true }).waitFor();
  assert.equal(await all.isEnabled(), true);
  await all.click();
  await page.getByText('160 permissions on 80 paths', { exact: true }).waitFor();
  if (process.env.BUTTERPOLLO_TEST_ARTIFACTS) {
    await page.screenshot({ path: path.join(process.env.BUTTERPOLLO_TEST_ARTIFACTS, 'token-all-desktop.png'), fullPage: true });
    await page.setViewportSize({ width: 390, height: 844 });
    assert(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
    await page.screenshot({ path: path.join(process.env.BUTTERPOLLO_TEST_ARTIFACTS, 'token-all-mobile.png'), fullPage: true });
  }
  await create.click();
  await page.getByLabel('New token', { exact: true }).waitFor();
  assert.deepEqual(writes, [{ path: '/api/token', body: { scopes: routes } }]);
  assert.equal(await page.title(), 'Rubylight');
  assert.equal(await page.locator('vite-error-overlay').count(), 0);
  assert.deepEqual(consoleErrors, []);
});

test('deleting an app with unsaved edits returns to the library without a second question', async t => {
  const { page, writes } = await fixture(t);
  await page.goto(base + '/library/one');
  await page.getByLabel('Name', { exact: true }).fill('Edited, then deleted');
  await page.getByRole('button', { name: 'Delete', exact: true }).click();
  await page.getByRole('dialog').getByRole('button', { name: 'Delete', exact: true }).click();
  await page.waitForURL(base + '/library');
  await page.getByText('Deleted One.', { exact: true }).waitFor();
  assert.equal(await page.getByRole('dialog').isVisible(), false);
  assert.equal(writes.filter(write => write.path.startsWith('/api/apps/') && write.path !== '/api/apps/reorder').length, 1);
});

test('a long list of signed-in browsers starts short, with this browser first', async t => {
  const sessions = Array.from({ length: 7 }, (_, index) => ({ id: `s${index}`, current: index === 4, username: 'admin',
    last_seen: 1000 - index, device_label: `Browser ${index}`, remote_address: '127.0.0.1' }));
  const { page } = await fixture(t, async (route, url) => {
    if (url.pathname !== '/api/auth/sessions') return false;
    await route.fulfill({ json: { status: true, sessions } });
    return true;
  });
  await page.goto(base + '/maintenance');
  const list = page.locator('main').getByRole('list').filter({ hasText: 'Browser 4' });
  await list.waitFor();
  assert.equal(await list.getByRole('listitem').count(), 5);
  assert.match(await list.getByRole('listitem').first().innerText(), /Browser 4\s*This browser/);
  await page.getByRole('button', { name: 'Show all 7 browsers', exact: true }).click();
  assert.equal(await list.getByRole('listitem').count(), 7);
});


test('installed imports filter by launcher, preserve selection and send only stable identities', async t => {
  const sources = [['steam', 'Steam'], ['epic', 'Epic Games'], ['battlenet', 'Battle.net'], ['ea', 'EA app'], ['ubisoft', 'Ubisoft Connect'], ['gog', 'GOG']]
    .map(([source, name]) => ({ source, name, state: 'available', errors: [] }));
  const games = sources.map((source, index) => ({ source: source.source, id: String(index), uuid: 'game-' + index, name: index < 2 ? 'Same title' : source.name + ' game', install_dir: 'E:/Games', has_cover: false, already_added: index === 2, unavailable_reason: index === 3 ? 'EA app is unavailable' : null }));
  const { page, writes } = await fixture(t, async (route, url, request) => {
    if (url.pathname === '/api/library/discovery') { await route.fulfill({ json: { status: true, sources, games } }); return true; }
    if (url.pathname !== '/api/library/import') return false;
    const results = request.postDataJSON().games.map(game => ({ ...game, result: game.source === 'epic' ? 'failed' : 'added', error: game.source === 'epic' ? 'Game was uninstalled. Scan again.' : null }));
    await route.fulfill({ json: { status: true, results } });
    return true;
  });
  await page.goto(base + '/library/import');
  await page.getByRole('heading', { name: 'Add games', exact: true }).waitFor();
  const steam = page.getByRole('checkbox', { name: 'Add Same title from Steam', exact: true });
  const epic = page.getByRole('checkbox', { name: 'Add Same title from Epic Games', exact: true });
  await steam.check();
  await page.getByRole('combobox', { name: 'Launcher', exact: true }).selectOption('epic');
  await epic.check();
  await page.getByRole('searchbox', { name: 'Search games', exact: true }).fill('no match');
  await page.getByRole('heading', { name: 'No matching games' }).waitFor();
  await page.getByRole('searchbox', { name: 'Search games', exact: true }).fill('');
  await page.getByRole('combobox', { name: 'Launcher', exact: true }).selectOption('all');
  assert.equal(await steam.isChecked(), true);
  assert.equal(await page.getByRole('checkbox', { name: 'Add Battle.net game from Battle.net', exact: true }).isDisabled(), true);
  assert.equal(await page.getByRole('checkbox', { name: 'Add EA app game from EA app', exact: true }).isDisabled(), true);
  await page.getByRole('button', { name: 'Add selected games', exact: true }).click();
  await page.getByText('1 game added.', { exact: true }).waitFor();
  await page.getByText(/Same title: Game was uninstalled/).waitFor();
  assert.deepEqual(writes.find(write => write.path === '/api/library/import').body, { games: [{ source: 'steam', id: '0' }, { source: 'epic', id: '1' }] });
  assert.equal(await steam.isDisabled(), true);
  assert.equal(await epic.isChecked(), true, 'failed imports remain selected for retry');
  assert.equal(await page.getByRole('link', { name: 'Add manually', exact: true }).getAttribute('href'), '/library/new');
  await page.setViewportSize({ width: 390, height: 844 });
  assert(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
});

test('import scans distinguish missing launchers, empty libraries, partial errors and connection failures', async t => {
  let fail = false, scans = 0;
  const { page, writes } = await fixture(t, async (route, url) => {
    if (url.pathname !== '/api/library/discovery') return false;
    scans++;
    if (fail) await route.fulfill({ status: 503, body: 'Discovery unavailable' });
    else await route.fulfill({ json: { status: true, games: [], sources: [
      { source: 'steam', name: 'Steam', state: 'available', errors: [] },
      { source: 'epic', name: 'Epic Games', state: 'unavailable', errors: [] },
      { source: 'battlenet', name: 'Battle.net', state: 'error', errors: ['The product database could not be read.'] },
    ] } });
    return true;
  });
  await page.goto(base + '/library/import');
  await page.getByRole('heading', { name: 'No installed games found' }).waitFor();
  await page.getByText('Launcher not found', { exact: true }).waitFor();
  await page.getByText('No installed games', { exact: true }).waitFor();
  await page.getByText('Scan incomplete', { exact: true }).waitFor();
  assert.equal(scans, 1);
  fail = true;
  await page.getByRole('button', { name: 'Scan again', exact: true }).click();
  await page.getByRole('alert').filter({ hasText: 'Discovery unavailable' }).waitFor();
  assert.equal(await page.getByRole('button', { name: 'Add selected games', exact: true }).isDisabled(), true);
  fail = false;
  await page.getByRole('button', { name: 'Scan again', exact: true }).click();
  await page.getByRole('heading', { name: 'No installed games found' }).waitFor();
  assert.equal(scans, 3);
  assert.deepEqual(writes, []);
});
