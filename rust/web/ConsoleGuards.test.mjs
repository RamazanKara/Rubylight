import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';

const read = (path) => readFileSync(new URL(`./src/${path}`, import.meta.url), 'utf8');

test('a switch shows its controlled value until the caller changes it', () => {
  assert.match(read('components/settings/Switch.svelte'), /const next = event\.currentTarget\.checked;\s*event\.currentTarget\.checked = checked;\s*onchange\(next\);/);
});

test('a field error is announced and can be referenced by its control', () => {
  assert.match(read('components/Field.svelte'), /<p class="error" id=\{id \? `\$\{id\}-error` : undefined\} role="alert">/);
});

test('Settings follows ?q= when navigation changes it', () => {
  assert.match(read('pages/Settings.svelte'), /if \(next === urlQuery\) return;\s*urlQuery = next;\s*query = next \?\? '';/);
});

test('only the newest session refresh lands on the overview', () => {
  assert.match(read('pages/Overview.svelte'), /const id = \+\+latestSessions;[\s\S]*?if \(id !== latestSessions\) return;/);
});

test('a quick-switch reveal is cancelled by the next one and on unmount', () => {
  const source = read('components/QuickSwitcher.svelte');
  assert.match(source, /onDestroy\(\(\) => clearTimeout\(revealTimer\)\)/);
  assert.match(source, /function reveal\(name: string\) \{\s*clearTimeout\(revealTimer\);/);
});

test('a background poll keeps a device editor with unsaved changes open', () => {
  assert.match(read('pages/Devices.svelte'), /if \(editing && !editorDirty && !list\.some/);
});

test('a refresh reply that is not JSON signs out instead of throwing', () => {
  assert.match(read('lib/api.ts'), /try \{\s*body = JSON\.parse\(text\) as IssuedSession;\s*\} catch \{\s*return false;\s*\}/);
});

test('Settings can list only the settings changed from their defaults', () => {
  const settings = read('pages/Settings.svelte');
  assert.match(settings, /<span>Customized only<\/span>\s*<Switch checked=\{customizedOnly\} onchange=\{setCustomizedOnly\} \/>/);
  assert.match(settings, /const browsingResults = \$derived\(searching \|\| review !== 'all' \|\| customizedOnly\);/);
  assert.match(settings, /return setting !== undefined && differsFromDefault\(setting, edits, config\);/);
  assert.match(settings, /listingCustomized \? 'All settings are at their defaults'/);
  assert.match(settings, /review = 'all';\s*customizedOnly = false;\s*navigate\(`\/settings\/\$\{id\}`\);/);
});

test('the library loads what the host offers when opened directly', () => {
  assert.match(read('pages/Library.svelte'), /if \(!session\.metadata\) refreshMetadata\(\)\.catch\(\(\) => \{\}\);/);
});
