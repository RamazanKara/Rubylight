import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

const server = await createTestServer();
const draftModule = await server.ssrLoadModule('/src/components/library/draft.svelte.ts');
const files = await server.ssrLoadModule('/src/components/library/gamefiles.ts');
const { AppDraft, FRAME_LIMIT, STREAM_PRESET, frameLimitError } = draftModule;
const app = extra => ({ name: 'Game', cmd: 'game.exe', 'working-dir': '', 'prep-cmd': [], uuid: 'a', ...extra });
const limit = { path: 'OptiScaler.ini', section: 'Framerate', key: 'FramerateLimit', value: 60 };

test('an app without device settings sends none, and an added device with nothing set is not saved', () => {
  const draft = new AppDraft(app({}));
  assert.deepEqual(draft.devices(), []);
  draft.addDevice('phone');
  assert.deepEqual(draft.devices(), ['phone']);
  assert.equal(draft.changed, false);
  assert.ok(!('device-overrides' in draft.record()));
});

test('a device gets its own preset, frame limit, Radeon preset and game files', () => {
  const draft = new AppDraft(app({ 'radeon-profile': 'quality', 'game-files': [{ ...limit, value: '{stream_fps}' }] }));
  draft.addDevice('tv');
  draft.setDeviceSetting('tv', STREAM_PRESET, 'reference');
  draft.setDeviceSetting('tv', FRAME_LIMIT, 59.94);
  draft.setDeviceRadeon('tv', 'smoothness');
  draft.deviceFiles.tv.push(files.gameFileRow(limit));
  assert.equal(draft.invalid, false);
  assert.deepEqual(draft.record()['device-overrides'], {
    tv: {
      'config-overrides': { stream_preset: 'reference', frame_limiter_fps_limit: 59.94 },
      'radeon-profile': 'smoothness',
      'game-files': [limit],
    },
  });
  // The app's own stay as they were.
  assert.equal(draft.record()['radeon-profile'], 'quality');
  assert.equal(draft.record()['game-files'][0].value, '{stream_fps}');
  // Off is a choice of its own; Same as the game removes the key.
  draft.setDeviceRadeon('tv', '');
  assert.equal(draft.record()['device-overrides'].tv['radeon-profile'], '');
  draft.setDeviceRadeon('tv', null);
  assert.ok(!('radeon-profile' in draft.record()['device-overrides'].tv));
  // Clearing everything drops the block, and with it the key.
  draft.setDeviceSetting('tv', STREAM_PRESET, undefined);
  draft.setDeviceSetting('tv', FRAME_LIMIT, '');
  draft.deviceFiles.tv.splice(0, 1);
  assert.ok(!('device-overrides' in draft.record()));
});

test('blocks of devices that are not paired and settings the page does not show are kept', () => {
  const stored = {
    gone: { 'config-overrides': { max_bitrate: 20000 } },
    phone: {
      'config-overrides': { stream_preset: 'lowest_latency', max_bitrate: 30000 },
      'radeon-profile': 'custom',
      'radeon-settings': { 'anti_lag.enabled': true },
      'game-files': [limit],
    },
  };
  const draft = new AppDraft(app({ 'device-overrides': stored }));
  assert.equal(draft.changed, false);
  assert.deepEqual(draft.record()['device-overrides'], stored);
  assert.deepEqual(draft.deviceFiles.phone.map(row => row.key), ['FramerateLimit']);
  draft.setDeviceSetting('phone', STREAM_PRESET, 'reference');
  const phone = draft.record()['device-overrides'].phone;
  assert.equal(phone['config-overrides'].max_bitrate, 30000);
  assert.deepEqual(phone['radeon-settings'], { 'anti_lag.enabled': true });
  // A preset replaces a custom profile's settings.
  draft.setDeviceRadeon('phone', 'competitive');
  assert.ok(!('radeon-settings' in draft.record()['device-overrides'].phone));
  draft.removeDevice('phone');
  assert.deepEqual(draft.record()['device-overrides'], { gone: stored.gone });
  assert.deepEqual(draft.devices(), ['gone']);
});

test('a frame limit or a game file the host would refuse blocks saving', () => {
  for (const good of [undefined, '', 0, 60, 59.94, '119.880', 1000]) assert.equal(frameLimitError(good), '', String(good));
  for (const bad of ['59.', 'abc', -1, 1000.5, '60.1234', 1e6]) assert.notEqual(frameLimitError(bad), '', String(bad));
  const draft = new AppDraft(app({}));
  draft.addDevice('tv');
  draft.setDeviceSetting('tv', FRAME_LIMIT, '59.');
  assert.equal(draft.invalid, true);
  draft.setDeviceSetting('tv', FRAME_LIMIT, 60);
  assert.equal(draft.invalid, false);
  draft.deviceFiles.tv.push(files.gameFileRow({ ...limit, path: '..\\x.ini' }));
  assert.match(draft.devicesError, /\.\./);
  assert.equal(draft.invalid, true);
});

test('reset reads the device blocks again', () => {
  const draft = new AppDraft(app({}));
  draft.addDevice('tv');
  draft.deviceFiles.tv.push(files.gameFileRow(limit));
  assert.equal(draft.changed, true);
  draft.reset(draft.saved);
  assert.deepEqual(draft.devices(), []);
  assert.deepEqual(draft.deviceFiles, {});
  assert.equal(draft.changed, false);
});
