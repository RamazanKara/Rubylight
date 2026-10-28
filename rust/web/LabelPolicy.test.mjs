import assert from 'node:assert/strict';
import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join } from 'node:path';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

// The label policy (ux/labels.md): no setting, page or badge says Experimental; features ship plainly and keep
// their keys. Identifiers such as experimental_packed_444, amdvce_experimental or a wake target's
// 'experimental' state are not text, and comments are not shown.
const server = await createTestServer({ configFile: false });
const { schema } = await server.ssrLoadModule('/src/lib/settings-schema.ts');
const { settings: pending } = await server.ssrLoadModule('/src/lib/schema/extensions.ts');
const { PERMISSION_GROUPS } = await server.ssrLoadModule('/src/components/devices/device.ts');
await server.close();

const all = [...schema.settings, ...pending];
const texts = setting => [
  setting.label,
  setting.description,
  setting.details,
  setting.inactiveHint,
  ...(setting.control?.options ?? []).map(option => option.label),
].filter(text => typeof text === 'string');

const files = dir => readdirSync(dir).flatMap(name => {
  const path = join(dir, name);
  return statSync(path).isDirectory() ? files(path) : /\.(svelte|ts)$/.test(name) ? [path] : [];
});
const IDENTIFIER = /\w+_experimental\w*|experimental_\w+|\.experimental\b|\bexperimental:|'experimental'/gi;
const shown = source => source
  .replace(/\/\*[\s\S]*?\*\//g, '')
  .replace(/<!--[\s\S]*?-->/g, '')
  .replace(/(^|[^:])\/\/.*$/gm, '$1')
  .replace(IDENTIFIER, '');

test('no setting text says Experimental, in any case', () => {
  for (const setting of all) {
    for (const text of texts(setting)) assert.doesNotMatch(text, /experiment/i, `${setting.key}: ${text}`);
  }
  for (const item of PERMISSION_GROUPS.flatMap(group => group.items)) assert.doesNotMatch(item.label, /experiment/i);
});

test('no page, component or badge of the console says Experimental', () => {
  const src = new URL('./src', import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, '$1');
  const labelled = files(src).filter(path => /experiment/i.test(shown(readFileSync(path, 'utf8'))));
  assert.deepEqual(labelled, []);
});

test('no fork or credit names in setting text; Moonlight alone is the client', () => {
  for (const setting of all) {
    for (const text of texts(setting)) {
      assert.doesNotMatch(text, /Sunshine|Apollo|Vibepollo|Artemis|Foundation|Moonlight V\+/, `${setting.key}: ${text}`);
    }
  }
});
