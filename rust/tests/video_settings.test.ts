import assert from 'node:assert/strict';
import test from 'node:test';
import { NATIVE_ENCODERS } from '../web/src/lib/settings-types.ts';
import { settings } from '../web/src/lib/schema/video.ts';

const find = (key: string) => {
  const setting = settings.find((setting) => setting.key === key);
  assert.ok(setting, key);
  return setting;
};
const visible = (key: string, encoder?: string, gpus?: string[]) =>
  find(key).visibleWhen?.({ ...(encoder === undefined ? {} : { encoder }), ...(gpus ? { [NATIVE_ENCODERS]: gpus } : {}) }) ?? true;

test('AMD AMF settings show for AMF, and for Automatic on AMD or unknown GPUs', () => {
  for (const encoder of ['amf', 'amdvce', 'amdvce_experimental', 'amdvce_ffmpeg', 'amdvce_legacy']) {
    assert.equal(visible('amd_quality', encoder, ['nvenc']), true, `encoder ${encoder}`);
  }
  for (const encoder of [undefined, '', 'auto', 'pyrowave']) {
    assert.equal(visible('amd_quality', encoder), true, `encoder ${encoder}`);
    assert.equal(visible('amd_quality', encoder, ['amf']), true, `encoder ${encoder}`);
    assert.equal(visible('amd_quality', encoder, ['nvenc']), false, `encoder ${encoder}`);
  }
  assert.equal(visible('amd_quality', 'nvenc', ['amf']), false);
});

test('NVIDIA NVENC settings show for NVENC, and for Automatic on NVIDIA GPUs', () => {
  for (const encoder of ['nvenc', 'nvenc_experimental', 'nvenc_legacy']) {
    assert.equal(visible('nvenc_preset', encoder, ['amf']), true, `encoder ${encoder}`);
  }
  assert.equal(visible('nvenc_preset', 'auto', ['nvenc']), true);
  assert.equal(visible('nvenc_preset', 'auto', ['amf']), false);
  assert.equal(visible('nvenc_preset', 'auto'), false);
  assert.equal(visible('nvenc_preset', 'amf', ['nvenc']), false);
});

test('the encoder setting offers Automatic, AMD AMF, NVIDIA NVENC and PyroWave', () => {
  const encoder = find('encoder');
  assert.ok(encoder.control.kind === 'select');
  assert.deepEqual(encoder.control.options.map((option) => option.value), ['auto', 'amf', 'nvenc', 'pyrowave']);
  assert.deepEqual(encoder.aliases?.spellings, {
    amdvce: 'amf',
    amdvce_experimental: 'amf',
    amdvce_ffmpeg: 'amf',
    amdvce_legacy: 'amf',
    nvenc_experimental: 'nvenc',
    nvenc_legacy: 'nvenc',
  });
});

test('no settings remain for the removed encoders', () => {
  const removed = /^(qsv_|sw_)|^(min_threads|crf|qp)$/;
  assert.deepEqual(settings.filter((setting) => removed.test(setting.key)).map((setting) => setting.key), []);
  assert.deepEqual([...new Set(settings.filter((setting) => setting.category === 'encoders').map((setting) => setting.group))].sort(), ['AMD AMF', 'Encoder', 'NVIDIA NVENC']);
});
