import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

const server = await createTestServer();
const { AppDraft, CAPTURE_WINDOW } = await server.ssrLoadModule('/src/components/library/draft.svelte.ts');
const { schema } = await server.ssrLoadModule('/src/lib/settings-schema.ts');
const app = extra => ({ name: 'Game', cmd: 'game.exe', 'working-dir': '', 'prep-cmd': [], uuid: 'a', ...extra });
const settings = new Map(schema.settings.map(setting => [setting.key, setting]));

test('an app streams its window with its own match rules, and empty choices use the host setting', () => {
  const draft = new AppDraft(app({}));
  draft.setText('capture-window', 'window');
  draft.setText('capture-window-exe', 'game.exe');
  draft.setText('capture-window-title', 'Main');
  draft.setText('capture-window-closed', 'end');
  const record = draft.record();
  assert.equal(record['capture-window'], 'window');
  assert.equal(record['capture-window-exe'], 'game.exe');
  assert.equal(record['capture-window-title'], 'Main');
  assert.equal(record['capture-window-closed'], 'end');
  for (const key of ['capture-window', 'capture-window-exe', 'capture-window-title', 'capture-window-closed']) {
    draft.setText(key, '');
  }
  for (const key of ['capture-window', 'capture-window-exe', 'capture-window-title', 'capture-window-closed']) {
    assert.ok(!(key in draft.record()), key);
  }
});

test('a device chooses the display or the window for one app', () => {
  const draft = new AppDraft(app({ 'capture-window': 'window' }));
  draft.addDevice('tv');
  draft.setDeviceSetting('tv', CAPTURE_WINDOW, 'display');
  assert.deepEqual(draft.record()['device-overrides'], { tv: { 'config-overrides': { capture_window: 'display' } } });
  draft.setDeviceSetting('tv', CAPTURE_WINDOW, undefined);
  assert.ok(!('device-overrides' in draft.record()));
});

test('the host setting streams the display by default and its window options follow it', () => {
  const what = settings.get('capture_window');
  assert.equal(what.default, 'display');
  assert.equal(what.group, 'Capture');
  assert.deepEqual(what.control.options.map(option => option.value), ['display', 'window']);
  assert.equal(settings.get('capture_window_closed').default, 'display');
  for (const key of ['capture_window_closed', 'capture_window_exe', 'capture_window_title']) {
    const setting = settings.get(key);
    assert.equal(setting.visibleWhen({}), false, key);
    assert.equal(setting.visibleWhen({ capture_window: 'window' }), true, key);
    assert.equal(setting.visibleWhen({ capture_window: 'Display' }), false, key);
  }
});
