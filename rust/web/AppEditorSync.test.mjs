import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
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
const source = readFileSync(new URL('./src/components/library/AppEditor.svelte', import.meta.url), 'utf8');
const hint = /Started by ([\w. ]+)\. A command of your own replaces that\./;

test('a launcher\u2019s game says which launcher starts it, where the command is', () => {
  const epic = { 'launcher-source': 'epic', 'launcher-id': 'ns:item:Fortnite', 'synced-by': 'launcher', 'launcher-managed': 'auto' };
  assert.equal(editor(epic).match(hint)?.[1], 'Epic Games');
  assert.equal(editor({ 'launcher-source': 'battlenet', 'launcher-id': 'pro' }).match(hint)?.[1], 'Battle.net');
  assert.equal(editor({ 'synced-by': 'steam', 'steam-managed': 'auto', 'steam-id': '620' }).match(hint)?.[1], 'Steam');
  assert.equal(editor({ 'synced-by': 'playnite', 'playnite-id': 'aa' }).match(hint)?.[1], 'Playnite');
  // A game folder's program and the user's own programs start themselves.
  for (const app of [{ 'launcher-source': 'local', 'launcher-id': 'd:/games/hades', cmd: 'D:/Games/Hades/Hades.exe' }, { cmd: 'notepad.exe' }]) {
    const html = editor(app);
    assert.doesNotMatch(html, hint);
    assert.match(html, /Browse for a program, or leave this empty to stream the Desktop\./);
  }
});

test('saving a synced app is told in the toast, with no banner on the page', () => {
  for (const app of [{ 'synced-by': 'steam', 'steam-managed': 'auto' }, { 'synced-by': 'launcher', 'launcher-source': 'ubisoft', 'launcher-id': '1' }]) {
    assert.doesNotMatch(editor(app), /Saving keeps this game|will no longer rename or remove it/);
  }
  assert.match(source, /`Saved \$\{record\.name\.trim\(\)\}\. The \$\{handedOver\} sync keeps it as you saved it\.`/);
  assert.match(source, /if \(sync === 'launcher'\) return launcher === 'local' \? 'game folder' : launcherName/);
});

test('the editor links to All settings \u203a Games for the global defaults', () => {
  assert.match(editor({}), /<a href="\/settings\/games#app-behavior"[^>]*>All settings \u203a Games<\/a>/);
  assert.doesNotMatch(source, /Games & automation|Settings \u2192/);
});

test('the save bar shows only while a change waits; launching and deleting sit in the page', () => {
  // A saved game with nothing edited: no bar to float over the page.
  const clean = editor({});
  assert.doesNotMatch(clean, /<footer class="bar/);
  assert.doesNotMatch(clean, />Save<|>Discard</);
  assert.match(clean, /<div class="game-actions[^"]*">[\s\S]*Launch on PC[\s\S]*Delete[\s\S]*<\/div>/);
  // A new app has nothing to launch or delete, and keeps Save in place.
  const fresh = editor({ uuid: '' });
  assert.match(fresh, /<footer class="bar[^"]*"/);
  assert.doesNotMatch(fresh, /game-actions|Launch on PC/);
  assert.match(source, /\{#if draft\.changed \|\| isNew\}\s*<footer class="bar" class:dirty=\{draft\.changed\}>/);
  // Only the bar with a change floats.
  assert.match(source, /\.bar\.dirty \{\s*position: sticky;/);
  assert.doesNotMatch(source.match(/\n  \.bar \{[^}]*\}/)[0], /sticky/);
  assert.doesNotMatch(source.match(/<footer class="bar"[\s\S]*?<\/footer>/)[0], /Launch on PC|Delete|Close app/);
});
