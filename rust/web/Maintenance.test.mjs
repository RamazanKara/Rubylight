import assert from 'node:assert/strict';
import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join } from 'node:path';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

globalThis.location = new URL('http://localhost/');
globalThis.window = new EventTarget();
const server = await createTestServer();
const { statusLoader } = await server.ssrLoadModule('/src/components/maintenance/status-loader.ts');
const { render } = await server.ssrLoadModule('svelte/server');
const Banner = (await server.ssrLoadModule('/src/components/maintenance/ChildSessionBanner.svelte')).default;
const { api, setCsrfToken } = await server.ssrLoadModule('/src/lib/api.ts');
const { settings } = await server.ssrLoadModule('/src/lib/schema/display.ts');

const html = (initial) => render(Banner, { props: { initial } }).body;
const TITLE = /Windows settings from the removed child-session feature are still changed/;

test('the child-session banner shows only while Windows settings are still changed', () => {
  assert.doesNotMatch(html({ pending: false, problem: null }), TITLE);
  assert.doesNotMatch(html({ pending: false, problem: null }), /Restore again/);
  const out = html({ pending: true, problem: 'Child sessions were removed from Rubylight, and the Windows settings their setup changed were not restored: access denied.' });
  assert.match(out, TITLE);
  assert.match(out, /were not restored: access denied/);
  assert.match(out, /Restore again/);
  assert.match(out, /banner danger/);
});

test('Restore again posts to the restore endpoint and takes the new status', async () => {
  const seen = [];
  setCsrfToken('token');
  globalThis.fetch = async (path, init) => {
    seen.push({ path, method: init.method });
    return new Response('{"pending":false,"problem":null}', { status: 200 });
  };
  try {
    assert.deepEqual(await api.childSession.restore(), { pending: false, problem: null });
    assert.deepEqual(seen, [{ path: '/api/child-session/restore', method: 'POST' }]);
  } finally {
    delete globalThis.fetch;
  }
});

test('no password field is left outside the console sign-in', () => {
  const signIn = new Set(['Login.svelte', 'Setup.svelte', 'PasswordPanel.svelte']);
  const files = (folder) =>
    readdirSync(folder).flatMap((name) => {
      const path = join(folder, name);
      return statSync(path).isDirectory() ? files(path) : [path];
    });
  for (const path of files('src')) {
    if (!path.endsWith('.svelte') || signIn.has(path.split(/[\\/]/).pop())) continue;
    assert.doesNotMatch(readFileSync(path, 'utf8'), /type="password"/, path);
  }
});

test('no setting chooses a background mode: Nucleus Co-op is the only one', () => {
  for (const key of ['background_mode', 'child_session_resolution', 'child_session_scale', 'child_session_port']) {
    assert.equal(settings.find((setting) => setting.key === key), undefined, key);
  }
});

test('a poll tick waits for the status load in flight, and a slow load never overwrites a newer one', async () => {
  const pending = [];
  const applied = [];
  const failed = [];
  const loader = statusLoader(
    () => new Promise((resolve, reject) => pending.push({ resolve, reject })),
    (value) => applied.push(value),
    (error) => failed.push(error),
  );
  const slow = loader.load();
  await loader.tick();
  assert.equal(pending.length, 1, 'no second status request while one is in flight');
  // An action's own load still goes out, and its answer wins over the slower one.
  const asked = loader.load();
  assert.equal(pending.length, 2);
  pending[1].resolve('newer');
  await asked;
  pending[0].resolve('older');
  await slow;
  assert.deepEqual(applied, ['newer']);
  // Nothing in flight: the next tick loads.
  const next = loader.tick();
  assert.equal(pending.length, 3);
  pending[2].reject(new Error('host gone'));
  await next;
  assert.equal(failed.length, 1);
  assert.deepEqual(applied, ['newer']);
});
