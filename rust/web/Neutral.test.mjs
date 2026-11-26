/**
 * The console names no third-party component (AGENTS.md, GPL-3.0 section 7(e)): its own text says
 * "co-op splitter", and the host's reasons lose the name before they are shown.
 */
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createTestServer } from './test-server.mjs';

globalThis.location = new URL('http://localhost/');
globalThis.window = new EventTarget();
const server = await createTestServer();
const { neutral } = await server.ssrLoadModule('/src/lib/neutral.ts');
const { api } = await server.ssrLoadModule('/src/lib/api.ts');
test.after(() => server.close());

const NAMED = /Nucleus[ -]?Co-?op/i;

/** Source without its comments: what can reach the screen. */
function shown(source) {
  return source
    .replace(/<!--[\s\S]*?-->/g, '')
    .replace(/\/\*[\s\S]*?\*\//g, '')
    .replace(/(^|\s)\/\/.*$/gm, '$1');
}

function sources(dir) {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) return sources(path);
    return /\.(svelte|ts|html|css)$/.test(entry.name) ? [path] : [];
  });
}

test('no console text names the co-op splitter’s project', () => {
  const named = sources(fileURLToPath(new URL('./src/', import.meta.url))).flatMap((path) =>
    shown(readFileSync(path, 'utf8'))
      .split('\n')
      .filter((line) => NAMED.test(line))
      .map((line) => `${path}: ${line.trim()}`),
  );
  assert.deepEqual(named, []);
});

test('the host’s reasons read the same without the name', () => {
  const said = [
    ['Nucleus Co-op is running Portal 2: end that session first, then choose the folder', 'The co-op splitter is running Portal 2: end that session first, then choose the folder'],
    ['Rubylight doesn\'t run Nucleus Co-op from D:\\Downloads', 'Rubylight doesn\'t run the co-op splitter from D:\\Downloads'],
    [
      'Nucleus Co-op sessions need the host to run as the Rubylight Windows service, which starts Nucleus Co-op as the signed-in user',
      'Co-op sessions need the host to run as the Rubylight Windows service, which starts the co-op splitter as the signed-in user',
    ],
    ['No Nucleus Co-op handler for other.exe: add one in Nucleus Co-op\'s handler hub', 'No co-op handler for other.exe: add one in the co-op splitter\'s handler hub'],
    ['Saving the Nucleus Co-op folder failed: disk full', 'Saving the co-op splitter folder failed: disk full'],
    ['the compatibility settings a Nucleus Co-op session changed', 'the compatibility settings a co-op session changed'],
    ['It stopped. Nucleus Co-op exited', 'It stopped. The co-op splitter exited'],
    ['D:\\Downloads holds no NucleusCoop.exe: choose the folder you unpacked Nucleus Co-op 2.4.2 into', 'D:\\Downloads holds no co-op splitter program: choose the folder you unpacked the co-op splitter 2.4.2 into'],
    ['The NucleusCoop.exe in D:\\Old isn\'t Nucleus Co-op 2.4.2\'s', 'The co-op splitter program in D:\\Old isn\'t the co-op splitter 2.4.2\'s'],
    ['Portal 2 uses anti-cheat', 'Portal 2 uses anti-cheat'],
  ];
  for (const [host, console] of said) assert.equal(neutral(host), console);
});

test('a folder or file keeps its name on the disk', () => {
  const path = 'C:\\Program Files\\Nucleus Co-op\\NucleusCoop.exe is marked as downloaded from the internet';
  assert.equal(neutral(path), path);
  assert.equal(neutral('D:/Games/NucleusCoop can be changed by S-1-5-11'), 'D:/Games/NucleusCoop can be changed by S-1-5-11');
  // The setting's key is the host's identifier, not a name the console gives.
  assert.equal(neutral('nucleus_path must be a folder'), 'nucleus_path must be a folder');
});

test('an error the host answers with reaches the console without the name', async () => {
  const fetched = globalThis.fetch;
  globalThis.fetch = async (input) => {
    if (String(input).endsWith('/api/csrf-token')) return new Response(JSON.stringify({ csrf_token: 't' }), { status: 200 });
    return new Response(JSON.stringify({ error: 'Nucleus Co-op is not ready: reading the owner of C:\\Nucleus: access denied' }), {
      status: 503,
      headers: { 'Content-Type': 'application/json' },
    });
  };
  try {
    await assert.rejects(api.coop.open(), (error) => {
      assert.equal(error.message, 'The co-op splitter is not ready: reading the owner of C:\\Nucleus: access denied');
      return true;
    });
  } finally {
    globalThis.fetch = fetched;
  }
});
