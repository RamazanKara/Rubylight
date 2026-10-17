import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { after } from 'node:test';
import { createServer } from 'vite';

// Node runs these suites in parallel processes. Each SSR server needs its own
// optimizer cache, and hmr:false alone does not disable Vite's WebSocket listener.
export async function createTestServer({ configFile } = {}) {
  const tempRoot = resolve(tmpdir());
  const cacheDir = resolve(await mkdtemp(join(tempRoot, 'rubylight-web-test-')));
  const removeCache = async () => {
    // Only remove the fresh directory owned by this server, directly under tempRoot.
    if (dirname(cacheDir) !== tempRoot) throw new Error('Test cache escaped its temporary root');
    await rm(cacheDir, { recursive: true, force: true });
  };
  let server;
  try {
    server = await createServer({
      configFile,
      cacheDir,
      server: { middlewareMode: true, hmr: false, ws: false },
      appType: 'custom',
      optimizeDeps: { noDiscovery: true, include: [] },
    });
  } catch (error) {
    await removeCache();
    throw error;
  }
  after(async () => {
    try {
      await server.close();
    } finally {
      await removeCache();
    }
  });
  return server;
}
