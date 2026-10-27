import assert from 'node:assert/strict';
import { test } from 'node:test';
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
