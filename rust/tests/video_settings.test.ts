import assert from 'node:assert/strict';
import test from 'node:test';
import { settings } from '../web/src/lib/schema/video.ts';

const amd = settings.filter((setting) => setting.group === 'AMD AMF');
const visible = (setting: (typeof settings)[number], encoder?: string) =>
  setting.visibleWhen?.(encoder === undefined ? {} : { encoder }) ?? true;

test('AMD AMF settings are shown whatever the encoder setting is', () => {
  const quality = amd.find((setting) => setting.key === 'amd_quality');
  assert.ok(quality);
  for (const encoder of [undefined, '', 'auto', 'pyrowave', 'amf', 'amdvce', 'amdvce_experimental', 'amdvce_ffmpeg', 'amdvce_legacy']) {
    assert.equal(visible(quality, encoder), true, `encoder ${encoder}`);
  }
});

test('the encoder setting offers only Automatic, AMD AMF and PyroWave', () => {
  const encoder = settings.find((setting) => setting.key === 'encoder');
  assert.ok(encoder && encoder.control.kind === 'select');
  assert.deepEqual(encoder.control.options.map((option) => option.value), ['auto', 'amf', 'pyrowave']);
  assert.deepEqual(encoder.aliases?.spellings, {
    amdvce: 'amf',
    amdvce_experimental: 'amf',
    amdvce_ffmpeg: 'amf',
    amdvce_legacy: 'amf',
  });
});

test('no settings remain for the removed encoders', () => {
  const removed = /^(nvenc_|qsv_|sw_)|^(min_threads|crf|qp)$/;
  assert.deepEqual(settings.filter((setting) => removed.test(setting.key)).map((setting) => setting.key), []);
  assert.deepEqual([...new Set(settings.filter((setting) => setting.category === 'encoders').map((setting) => setting.group))].sort(), ['AMD AMF', 'Encoder']);
});
