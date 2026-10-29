import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

const server = await createTestServer();
const { AppDraft } = await server.ssrLoadModule('/src/components/library/draft.svelte.ts');
const files = await server.ssrLoadModule('/src/components/library/gamefiles.ts');
const app = extra => ({ name: 'Game', cmd: 'game.exe', 'working-dir': '', 'prep-cmd': [], uuid: 'a', ...extra });
const limit = { path: 'OptiScaler.ini', section: 'Framerate', key: 'FramerateLimit', value: '{stream_fps-1}' };

test('an app without game files is unchanged and sends none', () => {
  const draft = new AppDraft(app({}));
  assert.equal(draft.changed, false);
  assert.ok(!('game-files' in draft.record()));
  // A blank row is dropped, not an error.
  draft.files.push(files.gameFileRow());
  assert.equal(draft.changed, false);
  assert.equal(draft.invalid, false);
});

test('a stored list reads back unchanged, numbers stay numbers', () => {
  const stored = [limit, { path: 'Game\\Engine.ini', section: '', key: 'MaxFPS', value: 60 }];
  const draft = new AppDraft(app({ 'game-files': stored }));
  assert.equal(draft.changed, false);
  assert.deepEqual(draft.record()['game-files'], stored);
  draft.files[1].value = '90';
  assert.equal(draft.record()['game-files'][1].value, 90);
  draft.files[1].value = '059';
  assert.equal(draft.record()['game-files'][1].value, '059');
  draft.files.splice(0, 2);
  assert.ok(!('game-files' in draft.record()));
});

test('fields of an entry the editor does not show are kept', () => {
  const stored = [{ ...limit, note: 'OptiScaler 0.7' }];
  const draft = new AppDraft(app({ 'game-files': stored }));
  assert.equal(draft.changed, false);
  draft.files[0].value = '{stream_fps}';
  assert.deepEqual(draft.record()['game-files'], [{ ...limit, value: '{stream_fps}', note: 'OptiScaler 0.7' }]);
});

test('a row the host would refuse names the problem and blocks saving', () => {
  const draft = new AppDraft(app({}));
  draft.files.push(files.gameFileRow(limit));
  assert.equal(draft.invalid, false);
  const row = draft.files[0];
  for (const [field, value, why] of [
    ['path', '..\\x.ini', /without \.\./],
    ['path', '\\\\server\\share\\x.ini', /Network/],
    ['path', 'C:x.ini', /colon/],
    ['path', 'settings.cfg', /\.ini/],
    ['path', '', /Name the file/],
    ['section', '[Framerate]', /without \[ and \]/],
    ['key', '', /Name the key/],
    ['key', 'a=b', /before the =/],
    ['value', '', /value/],
    ['value', '{base_fps}', /stream_fps/],
    ['value', '{stream_fps*2}', /stream_fps/],
    ['value', '{stream_fps-1001}', /at most 1000/],
  ]) {
    const before = row[field];
    row[field] = value;
    assert.match(files.gameFileError(row, draft.files), why, `${field}=${value}`);
    assert.equal(draft.invalid, true, `${field}=${value}`);
    row[field] = before;
  }
  for (const value of ['{stream_fps}', '{ stream_fps + 2 }', '60', 'true', 'Auto']) {
    row.value = value;
    assert.equal(files.gameFileError(row, draft.files), '', value);
  }
  assert.equal(files.pathError('C:\\Games\\RDR2\\OptiScaler.INI'), '');
  // The same file, section and key twice, with either slash and any case.
  draft.files.push(files.gameFileRow({ ...limit, path: 'optiscaler.ini', key: 'framerateLIMIT' }));
  assert.equal(files.gameFileError(draft.files[1], draft.files), 'Listed twice.');
});

test('at most 32 keys are listed', () => {
  const rows = Array.from({ length: 33 }, (_, index) => files.gameFileRow({ ...limit, key: `K${index}` }));
  assert.match(files.gameFilesError(rows), /at most 32/);
  assert.equal(files.gameFilesError(rows.slice(0, 32)), '');
});
