import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

// The router reads the page's address when the component loads.
globalThis.location = new URL('http://localhost/');
globalThis.window = new EventTarget();
const source = readFileSync(new URL('./src/components/library/LauncherSources.svelte', import.meta.url), 'utf8');
const marks = readFileSync(new URL('./src/components/library/LauncherMark.svelte', import.meta.url), 'utf8');
const server = await createTestServer();
const sources = await server.ssrLoadModule('/src/components/library/LauncherSources.svelte');
const { schema, groupAnchor } = await server.ssrLoadModule('/src/lib/settings-schema.ts');
const { LAUNCHER_GROUPS } = await server.ssrLoadModule('/src/lib/schema/library.ts');

test('every launcher’s Options sits on its name line, so its status line keeps one line on desktop', () => {
  const name = source.match(/<span class="name">([\s\S]*?)<\/span>\s*<span class="state/);
  assert.ok(name, 'the launcher name line comes before its status line');
  assert.match(name[1], /\{#if key\}<a class="choose" href=\{optionsHref\(source\.source\)\} use:link aria-label="\{source\.name\} options">Options<\/a>/);
  // Nothing between the text column and the switch takes the status line's width.
  assert.doesNotMatch(source, /<\/span>\s*\{#if source\.source === 'steam'\}<a class="choose"/);
});

test('each launcher’s Options opens its own group on All settings › Games', () => {
  const groups = new Set(schema.settings.filter((setting) => setting.category === 'library').map((setting) => setting.group));
  for (const launcher of [...sources.LAUNCHER_ORDER, 'playnite']) {
    const group = LAUNCHER_GROUPS[launcher];
    assert.ok(groups.has(group), `${launcher}: ${group} is a group of the Games tab`);
    assert.equal(sources.optionsHref(launcher), `/settings/games#${groupAnchor(group)}`);
  }
  assert.equal(sources.optionsHref('epic'), '/settings/games#epic-games-options');
  assert.equal(sources.optionsHref('local'), '/settings/games#game-folders');
  assert.equal(sources.optionsHref('playnite'), '/settings/games#playnite');
  // Every launcher that syncs by itself has its own Sync automatically, as Steam does.
  const keys = new Set(schema.settings.map((setting) => setting.key));
  for (const key of Object.values(sources.AUTO_SYNC_KEYS)) assert.ok(keys.has(key), key);
});

test('a launcher the PC does not have shows its switch off and disabled', () => {
  const missing = { source: 'gog', name: 'GOG', state: 'unavailable' };
  assert.equal(sources.notInstalled(missing, []), true);
  assert.deepEqual(sources.describe(missing, []), { text: 'Not installed on this PC', tone: 'muted' });
  // A launcher that could not be read but has games is installed.
  assert.equal(sources.notInstalled(missing, [{ source: 'gog', id: '1', name: 'Gwent' }]), false);
  assert.equal(sources.notInstalled({ ...missing, state: 'available' }, []), false);
  assert.match(source, /checked=\{source\.state !== 'disabled' && !absent\}\s*disabled=\{absent\}/);
  assert.match(source, /title=\{absent \? `\$\{source\.name\} is not installed on this PC`/);
});

test('launcher tiles show each launcher’s mark, not letters', () => {
  assert.match(source, /<LauncherMark source=\{source\.source\} \/>/);
  assert.match(source, /<LauncherMark source="playnite" \/>/);
  assert.doesNotMatch(source, /MONOGRAM|monogram/);
  // Game folders take the last branch, a folder.
  for (const launcher of [...sources.LAUNCHER_ORDER.filter((id) => id !== 'local'), 'playnite']) {
    assert.match(marks, new RegExp(`\\{(#|:else )if source === '${launcher}'\\}`), launcher);
  }
  assert.doesNotMatch(marks, /<text\b/, 'marks are drawn, never lettered');
});

test('Playnite shows once, in the launcher list, and keeps its plugin, options and version', () => {
  const page = readFileSync(new URL('./src/pages/Library.svelte', import.meta.url), 'utf8');
  assert.doesNotMatch(page, /PlayniteSource/, 'no second Playnite bar under the panel');
  assert.match(source, /api\.playnite\.install\(\)/);
  assert.match(source, /'Update plugin' : 'Install plugin'/);
  assert.match(source, /href=\{optionsHref\('playnite'\)\} use:link aria-label="Playnite options">Options<\/a>/);
  assert.match(source, /plugin \$\{playnite\.installed_version\}/);
  // The panel's Sync now still syncs Playnite while it runs.
  assert.match(source, /playnite\?\.active \? api\.playnite\.sync\(\)/);
});

test('the panel links to All settings › Games, not the old tab names', () => {
  assert.match(source, /<a href=\{GAMES_SETTINGS\} use:link>All settings › Games<\/a>/);
  assert.equal(sources.GAMES_SETTINGS, '/settings/games');
  for (const file of ['src/components/library/LauncherSources.svelte', 'src/pages/LibraryImport.svelte', 'src/components/library/AppEditor.svelte', 'src/components/library/LosslessSettings.svelte']) {
    const text = readFileSync(new URL(`./${file}`, import.meta.url), 'utf8');
    assert.doesNotMatch(text, /Games & automation|Settings → (Games|Library)/, file);
  }
});
