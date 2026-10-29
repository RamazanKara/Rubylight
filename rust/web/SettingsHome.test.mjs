import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

const server = await createTestServer({ configFile: false });
const { schema, categoriesOf, settingsGroup } = await server.ssrLoadModule('/src/lib/settings-schema.ts');
const main = await server.ssrLoadModule('/src/lib/schema/main.ts');
const { searchSettings } = await server.ssrLoadModule('/src/components/settings/search.ts');
const { settings: extensions } = await server.ssrLoadModule('/src/lib/schema/extensions.ts');
const { describeDefault } = await server.ssrLoadModule('/src/components/settings/values.ts');
const { MAIN_SECTIONS, MAIN_KEYS, MAIN_BUDGET, ALWAYS_ON, EXPERT_GROUPS, DEBUG_GROUP, onHome } = main;
const settings = new Map(schema.settings.map(setting => [setting.key, setting]));
const source = name => readFile(new URL(`./src/${name}`, import.meta.url), 'utf8');

test('every main key is a live schema setting, listed once', () => {
  assert.equal(new Set(MAIN_KEYS).size, MAIN_KEYS.length);
  for (const key of MAIN_KEYS) {
    const setting = settings.get(key);
    assert.ok(setting, `${key} is in the schema`);
    assert.notEqual(setting.pending, true, key);
    // The home's top level shows it without a parent choice turning it on first.
    assert.equal(setting.visibleWhen?.({}) ?? true, true, key);
    assert.equal(ALWAYS_ON.includes(key), false, key);
    assert.equal(EXPERT_GROUPS.has(setting.group), false, key);
  }
});

test('the Settings home stays within its budget of 12 settings', () => {
  assert.equal(MAIN_BUDGET, 12);
  assert.ok(MAIN_KEYS.length <= MAIN_BUDGET, `${MAIN_KEYS.length} main settings`);
  assert.deepEqual(MAIN_SECTIONS.map(section => section.title), ['This PC', 'Picture', 'Sound and controls', 'Games', 'Updates']);
});

test('main settings are safe in every option: nothing Experimental on the home', () => {
  for (const key of MAIN_KEYS) {
    const setting = onHome(settings.get(key), settings.get(key).default);
    const options = setting.control.kind === 'select' ? setting.control.options.map(option => option.label) : [];
    assert.doesNotMatch([setting.label, setting.description, ...options].join(' '), /Experimental/, key);
  }
});

test('the monitor layout reads as an outcome and never rewrites another layout', () => {
  const layout = settings.get('virtual_display_layout');
  const plain = onHome(layout, 'exclusive');
  assert.equal(plain.key, 'virtual_display_layout');
  assert.equal(plain.label, 'While streaming, your monitors');
  assert.deepEqual(plain.control.options.map(option => [option.value, option.label]), [['exclusive', 'Turn off'], ['extended', 'Stay on']]);
  // A layout chosen in All settings stays a choice under its own name.
  const isolated = onHome(layout, 'extended_isolated');
  assert.deepEqual(isolated.control.options.map(option => option.value), ['exclusive', 'extended', 'extended_isolated']);
  assert.match(isolated.control.options[2].label, /^Stay on: Isolate/);
  // Every value the home offers is one the host reads.
  for (const option of isolated.control.options) assert.ok(layout.control.options.some(item => item.value === option.value));
  assert.equal(layout.default, 'exclusive');
});

test('every schema key is reachable from search, and opens its own category', () => {
  for (const setting of schema.settings) {
    const found = searchSettings(schema.settings, categoriesOf, setting.key);
    assert.ok(found.includes(setting), `${setting.key} is found by its key`);
    const byLabel = searchSettings(schema.settings, categoriesOf, setting.label);
    assert.ok(byLabel.includes(setting), `${setting.key} is found by its label`);
    assert.ok(categoriesOf(setting).some(category => category.id === settingsGroup(setting.category)), setting.key);
  }
  assert.deepEqual(searchSettings(schema.settings, categoriesOf, '   '), []);
});

test('always-on keys appear only in Debug switches, described as for troubleshooting', () => {
  assert.deepEqual([...ALWAYS_ON].sort(), ['link_rate_adaptation', 'qr_pairing']);
  for (const key of ALWAYS_ON) {
    const setting = settings.get(key);
    assert.ok(setting, `${key} stays readable in All settings`);
    assert.equal(setting.group, DEBUG_GROUP, key);
    assert.equal(settingsGroup(setting.category), 'general', key);
    assert.match(setting.description, /only for troubleshooting/, key);
    assert.equal(setting.default, true, key);
    assert.equal(MAIN_KEYS.includes(key), false, key);
  }
  const debug = schema.settings.filter(setting => setting.group === DEBUG_GROUP).map(setting => setting.key);
  assert.deepEqual(debug.sort(), [...ALWAYS_ON].sort());
  // The key itself is untouched in the part file.
  assert.equal(extensions.find(setting => setting.key === 'qr_pairing').group, 'Pairing');
});

test('only the expert groups start collapsed, and each one exists', () => {
  assert.deepEqual([...EXPERT_GROUPS].sort(), ['AMD AMF', 'Capture troubleshooting', 'Console', DEBUG_GROUP, 'Ports and addresses', 'RTSS'].sort());
  const groups = new Set(schema.settings.map(setting => setting.group));
  for (const group of EXPERT_GROUPS) assert.ok(groups.has(group), group);
});

test('All settings has no second Advanced fold, and the home owns updates, power and sign-in', async () => {
  const page = await source('pages/Settings.svelte');
  assert.doesNotMatch(page, /class="advanced"/);
  assert.doesNotMatch(page, /setting\.advanced/);
  assert.match(page, /EXPERT_GROUPS\.has\(group\.name\)/);
  const help = await source('pages/Maintenance.svelte');
  for (const panel of ['UpdatesPanel', 'PowerPanel', 'SessionsPanel', 'PasswordPanel']) {
    assert.doesNotMatch(help, new RegExp(`<${panel}`), panel);
  }
  const home = await source('pages/SettingsHome.svelte');
  for (const panel of ['UpdatesPanel', 'PowerPanel', 'SessionsPanel', 'PasswordPanel', 'SaveBar']) {
    assert.match(home, new RegExp(`<${panel}`), panel);
  }
  // The home saves through the same PATCH of changed keys as All settings.
  assert.match(home, /computeChanges\(defaults, edits, stored, new Set\(covered\)\)/);
  assert.match(home, /api\.config\.patch\(body\)/);
  const app = await source('App.svelte');
  assert.match(app, /route\.path === '\/settings'\) return SettingsHome/);
  assert.match(app, /match\('\/settings\/:section'\)\) return Settings/);
  for (const name of ['pages/Settings.svelte', 'pages/Maintenance.svelte', 'components/devices/PairPanel.svelte']) {
    assert.doesNotMatch(await source(name), /maintenance#account/, name);
  }
});

test('a default says what it does, never just "Default"', () => {
  for (const setting of schema.settings) {
    assert.doesNotMatch(describeDefault(setting).text, /^Default/, setting.key);
  }
  assert.equal(describeDefault(settings.get('amd_vbaq')).text, 'Off for H.264, on for HEVC and AV1');
});

test('the wake status never spins silently: a slow first check says why, and the request has a deadline', async () => {
  const panel = await source('components/maintenance/PowerPanel.svelte');
  assert.match(panel, /<LoadState error=\{loadError\} what="the wake status" slow="[^"]+half a minute\."/);
  const state = await source('components/LoadState.svelte');
  assert.match(state, /setTimeout\(\(\) => \(lingering = true\), 3000\)/);
  // An explicit space: Svelte trims one at the start of an {#if} block.
  assert.match(state, /\{#if lingering\}\{' '\}\{slow\}\{\/if\}/);
  assert.match(await source('lib/api.ts'), /status: \(refresh = false\) => get<PowerStatus>\(`[^`]*`, ACTION_TIMEOUT_MS\)/);
});

test('full chroma applies when saved: no restart tag, text or notice', async () => {
  const packed = settings.get('experimental_packed_444');
  assert.notEqual(packed.restart, true);
  assert.doesNotMatch(`${packed.label} ${packed.description}`, /restart/i);
  // The restart notice follows the host's answer, which is false for a save of only live settings.
  for (const name of ['pages/Settings.svelte', 'pages/SettingsHome.svelte']) {
    assert.match(await source(name), /saved = result\.restart_required !== false;\n\s*if \(!saved\) notify\('Settings saved\.', 'ok'\);/, name);
  }
});
