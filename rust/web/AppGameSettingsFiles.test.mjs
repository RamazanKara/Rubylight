import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

const server = await createTestServer();
const { AppDraft, textRow } = await server.ssrLoadModule('/src/components/library/draft.svelte.ts');
const files = await server.ssrLoadModule('/src/components/library/settingsfiles.ts');
const app = extra => ({ name: 'Game', cmd: 'game.exe', 'working-dir': '', 'prep-cmd': [], uuid: 'a', ...extra });
const settings = '%LOCALAPPDATA%\\Game\\Saved\\Config\\GameUserSettings.ini';

test('an app without game settings files sends none, and a blank row changes nothing', () => {
  const draft = new AppDraft(app({}));
  assert.ok(!('game-settings-files' in draft.record()));
  draft.settingsFiles.push(textRow());
  assert.equal(draft.changed, false);
  assert.equal(draft.invalid, false);
});

test('a stored list reads back unchanged and new paths are trimmed', () => {
  const draft = new AppDraft(app({ 'game-settings-files': [settings] }));
  assert.equal(draft.changed, false);
  assert.deepEqual(draft.record()['game-settings-files'], [settings]);
  draft.settingsFiles.push(textRow('  D:\\Games\\Racer\\video.cfg '));
  assert.equal(draft.changed, true);
  assert.deepEqual(draft.record()['game-settings-files'], [settings, 'D:\\Games\\Racer\\video.cfg']);
  draft.settingsFiles.splice(0, 2);
  assert.ok(!('game-settings-files' in draft.record()), 'an emptied list is removed');
});

test('paths the host refuses cannot be saved', () => {
  for (const bad of ['relative\\x.ini', '\\\\server\\share\\x.ini', 'C:\\Games\\..\\x.ini', '%LOCALAPPDATA\\x.ini', 'C:\\Games\\', 'C:\\a:b.ini']) {
    assert.notEqual(files.settingsFileError(bad), '', bad);
    const draft = new AppDraft(app({}));
    draft.settingsFiles.push(textRow(bad));
    assert.equal(draft.invalid, true, bad);
  }
  for (const good of [settings, 'C:/Games/x.ini', '%USERPROFILE%\\Documents\\My Games\\Game\\Settings.json']) {
    assert.equal(files.settingsFileError(good), '', good);
  }
  assert.match(files.settingsFilesError(['C:\\a.ini', 'c:/A.INI']), /twice/);
});
