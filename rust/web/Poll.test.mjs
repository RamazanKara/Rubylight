import assert from 'node:assert/strict';
import { test } from 'node:test';
import { readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createTestServer } from './test-server.mjs';

const server = await createTestServer();
const { marked, poll } = await server.ssrLoadModule('/src/lib/format.ts');
const { api } = await server.ssrLoadModule('/src/lib/api.ts');

test('polled requests say so, so a page left open does not keep the PC awake', async () => {
  globalThis.document = { visibilityState: 'visible' };
  const seen = [];
  globalThis.fetch = async (path, init) => {
    seen.push({ path, poll: init.headers['X-Rubylight-Poll'] ?? null });
    return new Response('{"status":true}', { status: 200 });
  };
  try {
    let done;
    const finished = new Promise((resolve) => (done = resolve));
    const stop = poll(async () => {
      await api.metadata();
      done();
    }, 60_000);
    await finished;
    stop();
    // What the user asks for is not a poll.
    await api.metadata();
    // A page's own timer (the Updates panel) marks its reload too.
    await marked(() => api.updates.state());
    assert.deepEqual(seen, [
      { path: '/api/metadata', poll: '1' },
      { path: '/api/metadata', poll: null },
      { path: '/api/updates', poll: '1' },
    ]);
  } finally {
    delete globalThis.fetch;
    delete globalThis.document;
  }
});

test('no page polls the host from a bare timer: requests go through poll() or marked()', () => {
  const sources = (dir) =>
    readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
      const path = join(dir, entry.name);
      if (entry.isDirectory()) return sources(path);
      return /\.(svelte|ts)$/.test(entry.name) ? [path] : [];
    });
  for (const path of sources(fileURLToPath(new URL('./src', import.meta.url)))) {
    const text = readFileSync(path, 'utf8');
    // A bare timer only ticks a clock; anything it fetched would keep the PC awake and run in a hidden tab.
    for (const match of text.matchAll(/setInterval\(/g)) {
      assert.match(text.slice(match.index), /^setInterval\(\(\) => \(now = Date\.now\(\)\), 1000\)/, path);
    }
  }
  // The co-op card's device list is fetched after the status's await, past the poll's own mark.
  const overview = readFileSync(new URL('./src/pages/Overview.svelte', import.meta.url), 'utf8');
  assert.match(overview, /await marked\(\(\) => api\.clients\.list\(\)\)/);
});
