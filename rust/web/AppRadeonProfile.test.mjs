import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

const server = await createTestServer();
const { AppDraft } = await server.ssrLoadModule('/src/components/library/draft.svelte.ts');
const radeon = await server.ssrLoadModule('/src/components/library/radeon.ts');
const app = extra => ({ name: 'Game', cmd: 'game.exe', 'working-dir': '', 'prep-cmd': [], uuid: 'a', ...extra });

test('the editor offers the four profiles the host has', () => {
  assert.deepEqual(
    radeon.profiles.map(profile => profile.id),
    ['competitive', 'smoothness', 'quality', 'custom'],
  );
});

test('an app without a profile is unchanged and sends no Radeon keys', () => {
  const draft = new AppDraft(app({}));
  assert.equal(draft.changed, false);
  assert.equal(draft.radeonError, '');
  assert.ok(!('radeon-profile' in draft.record()) && !('radeon-settings' in draft.record()));
});

test('choosing a profile sets only radeon-profile, and None removes it', () => {
  const draft = new AppDraft(app({}));
  draft.setText('radeon-profile', 'smoothness');
  assert.equal(draft.changed, true);
  assert.equal(draft.record()['radeon-profile'], 'smoothness');
  assert.ok(!('radeon-settings' in draft.record()));
  draft.setText('radeon-profile', '');
  assert.equal(draft.changed, false);
});

test('a custom profile lists settings with typed values', () => {
  const draft = new AppDraft(app({}));
  draft.setText('radeon-profile', 'custom');
  assert.match(draft.radeonError, /at least one/);
  assert.equal(draft.invalid, true);
  draft.radeon.push(radeon.settingRow('frame_rate_target.enabled', 'true'));
  draft.radeon.push(radeon.settingRow('frame_rate_target.fps', '90'));
  draft.radeon.push(radeon.settingRow('vsync.mode', 'always_off'));
  assert.equal(draft.radeonError, '');
  assert.equal(draft.invalid, false);
  assert.deepEqual(draft.record()['radeon-settings'], {
    'frame_rate_target.enabled': true,
    'frame_rate_target.fps': 90,
    'vsync.mode': 'always_off',
  });
});

test('settings are dropped from the record unless the profile is custom', () => {
  const draft = new AppDraft(app({ 'radeon-profile': 'custom', 'radeon-settings': { 'a.b': 1 } }));
  assert.equal(draft.radeon.length, 1);
  assert.equal(draft.changed, false);
  draft.setText('radeon-profile', 'quality');
  assert.ok(!('radeon-settings' in draft.record()));
  // The rows are kept, so switching back does not lose them.
  draft.setText('radeon-profile', 'custom');
  assert.deepEqual(draft.record()['radeon-settings'], { 'a.b': 1 });
});

test('a bad row names the problem and blocks saving', () => {
  const draft = new AppDraft(app({ 'radeon-profile': 'custom', 'radeon-settings': { 'a.b': 1 } }));
  draft.radeon.push(radeon.settingRow('Anti Lag', 'true'));
  assert.match(radeon.rowError(draft.radeon[1], draft.radeon), /Radeon setting id/);
  assert.notEqual(draft.radeonError, '');
  assert.equal(draft.invalid, true);
  draft.radeon[1].id = 'a.b';
  assert.equal(radeon.rowError(draft.radeon[1], draft.radeon), 'Listed twice.');
  draft.radeon[1].id = 'c.d';
  draft.radeon[1].value = '';
  assert.match(radeon.rowError(draft.radeon[1], draft.radeon), /value/);
  // A blank row is dropped, not an error.
  draft.radeon[1].id = '';
  assert.equal(draft.radeonError, '');
  assert.deepEqual(draft.record()['radeon-settings'], { 'a.b': 1 });
});

test('values read back the way they were typed', () => {
  assert.equal(radeon.parseValue(' true '), true);
  assert.equal(radeon.parseValue('false'), false);
  assert.equal(radeon.parseValue('-3'), -3);
  assert.equal(radeon.parseValue('1.5'), '1.5');
  assert.equal(radeon.parseValue('enhance_app_settings'), 'enhance_app_settings');
  assert.equal(radeon.formatValue(true), 'true');
  assert.equal(radeon.formatValue(null), '');
});

test('a number that is not a whole number is an error, not a text value', () => {
  const row = value => radeon.settingRow('a.b', value);
  for (const bad of ['1.5', '-0.25', '1e3', '.5', '99999999999999999999']) {
    assert.match(radeon.rowError(row(bad), []), /whole number/, bad);
  }
  for (const good of ['3', '-3', 'true', 'false', 'enhance_app_settings', 'v2']) {
    assert.equal(radeon.rowError(row(good), []), '', good);
  }
  // It blocks saving a custom profile.
  const rows = [row('1.5')];
  assert.match(radeon.profileError('custom', rows), /whole number/);
});

test('an unknown stored profile is kept as written', () => {
  const draft = new AppDraft(app({ 'radeon-profile': 'fastest' }));
  assert.equal(draft.changed, false);
  assert.equal(draft.record()['radeon-profile'], 'fastest');
  assert.equal(radeon.profiles.some(profile => profile.id === radeon.profileOf('fastest')), false);
});
