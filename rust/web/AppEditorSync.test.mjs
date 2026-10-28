import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

globalThis.location = new URL('http://localhost/');
globalThis.window = new EventTarget();
const server = await createTestServer();
const { render } = await server.ssrLoadModule('svelte/server');
const AppEditor = (await server.ssrLoadModule('/src/components/library/AppEditor.svelte')).default;
const noop = async () => {};
const editor = (extra) =>
  render(AppEditor, {
    props: {
      app: { name: 'Portal 2', cmd: '', 'working-dir': '', 'prep-cmd': [], uuid: 'a', ...extra },
      position: null,
      status: null,
      onsaved: noop,
      onmove: noop,
      ondeleted: () => {},
      onstatus: () => {},
    },
  }).body;
const notice = /Saving keeps this game in your list for good: the (\w+) sync will no longer rename or remove it\./;

test('a synced app says that saving it hands it over from its sync', () => {
  assert.equal(editor({ 'synced-by': 'steam', 'steam-managed': 'auto' }).match(notice)?.[1], 'Steam');
  assert.equal(editor({ 'synced-by': 'launcher', 'launcher-managed': 'auto' }).match(notice)?.[1], 'launcher');
  assert.equal(editor({ 'synced-by': 'playnite', 'playnite-id': 'aa' }).match(notice)?.[1], 'Playnite');
});

test('an app no sync owns has no hand-over notice, whatever its marker', () => {
  // Vibepollo marks its synced apps too; without the stamp they are the user's.
  const html = editor({ 'steam-managed': 'auto', 'steam-id': '620' });
  assert.doesNotMatch(html, notice);
  assert.doesNotMatch(html, /replaces its name, command and cover/);
});
