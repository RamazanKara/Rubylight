import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';

const source = readFileSync(new URL('./src/components/library/LauncherSources.svelte', import.meta.url), 'utf8');

test("Steam options sits on Steam's name line, so its status line keeps one line on desktop", () => {
  const name = source.match(/<span class="name">([\s\S]*?)<\/span>\s*<span class="state/);
  assert.ok(name, 'the launcher name line comes before its status line');
  assert.match(name[1], /Steam options/);
  // Nothing between the text column and the switch takes the status line's width.
  assert.doesNotMatch(source, /<\/span>\s*\{#if source\.source === 'steam'\}<a class="choose"/);
});

test('Playnite shows once, in the launcher list, and keeps its plugin, options and version', () => {
  const page = readFileSync(new URL('./src/pages/Library.svelte', import.meta.url), 'utf8');
  assert.doesNotMatch(page, /PlayniteSource/, 'no second Playnite bar under the panel');
  assert.match(source, /api\.playnite\.install\(\)/);
  assert.match(source, /'Update plugin' : 'Install plugin'/);
  assert.match(source, /Playnite options<\/a>/);
  assert.match(source, /plugin \$\{playnite\.installed_version\}/);
  // The panel's Sync now still syncs Playnite while it runs.
  assert.match(source, /playnite\?\.active \? api\.playnite\.sync\(\)/);
});
