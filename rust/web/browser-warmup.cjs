// Loads the console once before a browser suite's first test. A cold Vite server
// transforms every module and may re-optimize its dependencies (reloading the
// page) on the first visit, which can outlast a test's 10-30 s timeout. The
// warm-up gets that out of the way with a generous timeout; API calls get an
// empty answer, and page errors here are ignored. It never contacts a host.
async function warmUp(browser, base, paths = ['/']) {
  const page = await browser.newPage();
  try {
    await page.route('**/*', route => {
      const url = new URL(route.request().url());
      if (url.origin !== base) return route.abort();
      if (url.pathname.startsWith('/api/')) return route.fulfill({ json: { status: true } });
      return route.continue();
    });
    for (const path of paths) {
      await page.goto(base + path, { timeout: 120_000, waitUntil: 'load' });
      await page.waitForLoadState('networkidle', { timeout: 120_000 }).catch(() => {});
    }
  } finally {
    await page.close();
  }
}

module.exports = { warmUp };
