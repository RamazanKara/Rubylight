import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

globalThis.location = new URL('http://localhost/');
globalThis.window = new EventTarget();
const server = await createTestServer();
const { schema, settingGroups, settingsGroup, tabOf, groupRank, groupAnchor } = await server.ssrLoadModule('/src/lib/settings-schema.ts');
const { EXPERT_GROUPS, DEBUG_GROUP } = await server.ssrLoadModule('/src/lib/schema/main.ts');
const { describeDefault, numberError, textError } = await server.ssrLoadModule('/src/components/settings/values.ts');
const { default: SettingControl } = await server.ssrLoadModule('/src/components/settings/SettingControl.svelte');
const { render } = await server.ssrLoadModule('svelte/server');
const settings = new Map(schema.settings.map((setting) => [setting.key, setting]));
const source = (name) => readFile(new URL(`./src/${name}`, import.meta.url), 'utf8');

test('All settings has six tabs, named as people look for them', () => {
  assert.deepEqual(settingGroups.map((tab) => [tab.id, tab.label]), [
    ['pc', 'This PC'], ['picture', 'Picture'], ['sound', 'Sound & controls'],
    ['games', 'Games'], ['network', 'Network'], ['advanced', 'Advanced'],
  ]);
  for (const tab of settingGroups) assert.doesNotMatch(`${tab.label} ${tab.description}`, /\bhost\b|Experimental/i, tab.id);
});

test('older tab and topic addresses open the tab that now holds their settings', () => {
  const expected = {
    general: 'pc', video: 'picture', encoders: 'picture', 'frame-limiting': 'picture', display: 'picture', hdr: 'picture',
    audio: 'sound', input: 'sound', library: 'games', commands: 'games', network: 'network', advanced: 'advanced',
  };
  for (const [legacy, tab] of Object.entries(expected)) {
    assert.equal(settingsGroup(legacy), tab, legacy);
    assert.equal(tabOf(legacy).id, tab, legacy);
  }
  // A tab's own id is itself; anything unknown opens the first tab rather than an empty page.
  for (const tab of settingGroups) assert.equal(tabOf(tab.id).id, tab.id);
  assert.equal(tabOf('no-such-tab').id, 'pc');
});

test('every setting lands in one tab, under a group that tab orders or folds', () => {
  for (const setting of schema.settings) {
    const tab = settingsGroup(setting.category);
    assert.ok(settingGroups.some((item) => item.id === tab), setting.key);
    const group = setting.group ?? '';
    assert.ok(groupRank(tab, group) < 1000 || EXPERT_GROUPS.has(group), `${setting.key}: ${tab} › ${group}`);
  }
  // Frame pacing and every video setting are part of the picture.
  for (const setting of schema.settings.filter((item) => item.group === 'Frame pacing' || item.category === 'video')) {
    assert.equal(settingsGroup(setting.category), 'picture', setting.key);
  }
  // Folded groups come after the open ones, Debug switches last.
  assert.ok(groupRank('picture', 'Remote monitors') < groupRank('picture', 'AMD AMF'));
  assert.ok(groupRank('advanced', 'Console') < groupRank('advanced', DEBUG_GROUP));
});

test('settings that moved sit where people look for them', () => {
  const at = (key) => [settingsGroup(settings.get(key).category), settings.get(key).group];
  assert.deepEqual(at('enable_pairing'), ['network', 'Pairing']);
  assert.deepEqual(at('sleep_after_idle_minutes'), ['pc', 'Sleep and wake']);
  assert.deepEqual(at('min_log_level'), ['advanced', 'Logging']);
  assert.equal(at('usb_forwarding')[0], 'sound');
  assert.equal(at('clipboard_image')[0], 'network');
  for (const key of ['origin_web_ui_allowed', 'csrf_allowed_origins', 'session_token_ttl_seconds']) {
    assert.equal(settingsGroup(settings.get(key).category), 'advanced', key);
  }
  for (const key of ['remote_monitor_terminate_on_first_request', 'remote_monitor_confirm_app_replacement']) {
    assert.deepEqual(at(key), ['games', 'App behavior'], key);
  }
  for (const key of ['nucleus_path', 'nucleus_coop_players']) assert.deepEqual(at(key), ['games', 'Play together'], key);
  assert.deepEqual(at('fallback_mode'), ['picture', 'Resolution and refresh rate']);
});

test('a section address is the group name in plain letters', () => {
  assert.equal(groupAnchor('Sleep and wake'), 'sleep-and-wake');
  assert.equal(groupAnchor('HDR'), 'hdr');
  assert.equal(groupAnchor('Mouse, pen and touch'), 'mouse-pen-and-touch');
  for (const tab of settingGroups) {
    const groups = [...new Set(schema.settings.filter((setting) => settingsGroup(setting.category) === tab.id).map((setting) => setting.group ?? ''))];
    const anchors = groups.map(groupAnchor);
    assert.equal(new Set(anchors).size, anchors.length, `${tab.id}: ${anchors}`);
  }
});

test('presets are valid values with their own names, and a default that is one says its name', () => {
  for (const setting of schema.settings) {
    const presets = setting.control.presets;
    if (!presets) continue;
    assert.ok(presets.length >= 2, setting.key);
    assert.equal(new Set(presets.map((preset) => preset.label)).size, presets.length, setting.key);
    assert.equal(new Set(presets.map((preset) => String(preset.value))).size, presets.length, setting.key);
    for (const preset of presets) {
      const error = setting.control.kind === 'number' ? numberError(setting, String(preset.value)) : textError(setting, preset.value);
      assert.equal(error, '', `${setting.key} = ${preset.value}`);
      assert.doesNotMatch(preset.label, /\bhost\b|Experimental/i, setting.key);
    }
  }
  assert.equal(describeDefault(settings.get('update_check_interval')).text, 'Daily');
  assert.equal(describeDefault(settings.get('sleep_after_idle_minutes')).text, 'Never');
  assert.deepEqual(settings.get('update_check_interval').control.presets.map((preset) => [preset.label, preset.value]),
    [['Every 6 hours', 21600], ['Daily', 86400], ['Weekly', 604800], ['Off', 0]]);
});

test('a preset value shows as its name; any other value as Custom with its own field', () => {
  const setting = settings.get('update_check_interval');
  const control = (value) => render(SettingControl, { props: { setting, id: setting.key, value, onchange: () => {} } }).body;
  const weekly = control(604800);
  assert.match(weekly, /<option value="604800"[^>]*selected[^>]*>(?:<!--[^>]*-->)?Weekly/);
  assert.doesNotMatch(weekly, /type="number"/);
  const custom = control(7200);
  assert.match(custom, /<option[^>]*selected[^>]*>Custom/);
  assert.match(custom, /type="number"/);
  assert.match(custom, /value="7200"/);
  assert.match(custom, /aria-label="Check for updates, custom value"/);
  // A text preset works the same way.
  const resolution = settings.get('dd_manual_resolution');
  const text = render(SettingControl, { props: { setting: resolution, id: resolution.key, value: '2560x1440', onchange: () => {} } }).body;
  assert.match(text, /<option value="2560x1440"[^>]*selected[^>]*>(?:<!--[^>]*-->)?2560 × 1440/);
  assert.doesNotMatch(text, /type="text"/);
});

test('settings written as JSON say so and fold under their own name', () => {
  for (const key of ['dd_mode_remapping', 'keybindings']) {
    const setting = settings.get(key);
    assert.equal(setting.control.kind, 'json', key);
    assert.match(setting.label, /\(JSON\)$/, key);
    assert.ok(EXPERT_GROUPS.has(setting.group), key);
    assert.equal(setting.label.replace(' (JSON)', ''), setting.group, key);
  }
});

test('the shared building blocks: one fold, finer points beside the label, cards as tall as their content', async () => {
  const panel = await source('components/Panel.svelte');
  assert.match(panel, /<Disclosure /);
  assert.match(panel, /\.title \{\s*font-size: var\(--text-md\);/);
  const section = await source('components/maintenance/Section.svelte');
  assert.match(section, /\.panels \{[^}]*align-items: start;/);
  assert.match(section, /h2 \{[^}]*font-size: var\(--text-lg\);/);
  const row = await source('components/settings/SettingRow.svelte');
  assert.match(row, /<InfoTip label=\{setting\.label\} text=\{setting\.details\} \/>/);
  assert.doesNotMatch(row, /<details/);
  const tip = await source('components/InfoTip.svelte');
  assert.match(tip, /width: var\(--control-height\);\s*height: var\(--control-height\);/);
  const settingsPage = await source('pages/Settings.svelte');
  assert.doesNotMatch(settingsPage, /<details/);
  assert.match(settingsPage, /class="contents" aria-label="\{active\.label\} sections"/);
});

test('the Settings home keeps the few things, with wake as one row', async () => {
  const home = await source('pages/SettingsHome.svelte');
  assert.match(home, /<WakeRow /);
  assert.doesNotMatch(home, /<(PowerPanel|SessionsPanel|PasswordPanel)\b/);
  const wake = await source('components/settings/WakeRow.svelte');
  // One click, the same adapter changes the detail panel lists, no question in between.
  assert.match(wake, /api\.power\.fixWake\(changes, false\)/);
  assert.doesNotMatch(wake, /confirm\(/);
  assert.doesNotMatch(settings.get('enable_pairing').description, /turn it off/i);
});

test('sign-in pages name no other streaming host and never hold their button back', async () => {
  const login = await source('pages/Login.svelte');
  assert.doesNotMatch(login, /Apollo|Vibepollo|Sunshine|this host/);
  for (const name of ['pages/Setup.svelte', 'components/maintenance/PasswordPanel.svelte']) {
    const page = await source(name);
    assert.match(page, /type="submit"/, name);
    assert.doesNotMatch(page, /disabled=\{/, name);
    assert.match(page, /novalidate/, name);
    assert.match(page, /aria-invalid=/, name);
  }
  assert.doesNotMatch(await source('pages/Setup.svelte'), /this host/);
});

test('Stream turns the WebRTC server on in one step', async () => {
  const stream = await source('pages/Stream.svelte');
  assert.doesNotMatch(stream, /Open the WebRTC setting/);
  assert.match(stream, /api\.config\.patch\(\{ webrtc_server: true \}\)/);
  assert.match(stream, /await api\.host\.restart\(\);\s*if \(!\(await backAfterRestart\(/);
  // A restart ends streams: it asks only while one runs.
  assert.match(stream, /activeSessions \?\? 0\) > 0 && !\(await confirmRestart\(\)\)/);
});
