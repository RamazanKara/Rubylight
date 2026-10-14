// Optional Playwright runtime, with every API request isolated from a real host.
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

function deferred() {
  let resolve;
  const promise = new Promise(done => { resolve = done; });
  return { promise, resolve };
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
    };
    return reply(replies[url.pathname] ?? { status: true, available: false, enabled: false });
  });
  return { page, writes };
}

test('maintenance panels recover failed loads without reloading the console', async t => {
  const failures = new Set(['/api/display/golden_status', '/api/health/crashdump', '/api/health/vulkan-hdr-layer',
    '/api/frame-limiter/status', '/api/auth/sessions', '/api/updates']);
  const { page } = await fixture(t, async (route, url) => {
    if (!failures.delete(url.pathname)) return false;
    await route.fulfill({ status: 503, body: 'Temporarily unavailable' });
    return true;
  });
  await page.goto(base + '/maintenance');
  for (const what of ['the display layout', 'crash reports', 'the Vulkan layer status',
    'the frame limiter status', 'signed-in browsers', 'the update status']) {
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
  await page.getByRole('button', { name: 'Disconnect First device', exact: true }).click();
  await page.getByRole('dialog').getByRole('button', { name: 'Disconnect', exact: true }).click();
  await started.promise;
  for (const name of ['Unpair all', 'Unpair First device', 'Edit First device', 'Unpair Second device', 'Edit Second device']) {
    assert.equal(await page.getByRole('button', { name, exact: true }).isDisabled(), true);
  }
  held.resolve();
  await page.getByText('Disconnected First device.', { exact: true }).waitFor();
  await page.getByRole('button', { name: 'Edit Second device', exact: true }).click();
  await page.getByRole('form', { name: 'Edit Second device', exact: true }).waitFor();
  assert.equal(writes.filter(write => write.path === '/api/clients/disconnect').length, 1);
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
  await page.getByRole('button', { name: 'Edit First device', exact: true }).click();
  await page.getByLabel('Name', { exact: true }).fill('New name');
  await page.getByRole('button', { name: 'Save', exact: true }).click();
  await started.promise;
  for (const name of ['Unpair all', 'Unpair First device', 'Close First device', 'Edit Second device']) {
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
  await page.getByRole('navigation', { name: 'Main' }).getByRole('link', { name: 'Library', exact: true }).click();
  await page.getByText('Saving the library order…', { exact: true }).waitFor();
  held.resolve();
  await page.getByText('Saving the order failed: Order not saved', { exact: true }).waitFor();
  await page.locator('[data-uuid="one"][draggable="true"]').waitFor();
  assert.deepEqual(await page.locator('[data-uuid]').evaluateAll(nodes => nodes.map(node => node.dataset.uuid)), ['one', 'two', 'three']);
  assert.equal(writes.filter(write => write.path === '/api/apps/reorder').length, 1);
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
  await page.getByRole('button', { name: 'Pair', exact: true }).click();
  await page.getByText('Pairing failed: pairing request no longer exists', { exact: true }).waitFor();
  await pin.waitFor({ state: 'detached' });
});
