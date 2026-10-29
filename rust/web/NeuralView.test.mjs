import assert from 'node:assert/strict';
import { test } from 'node:test';
import { readFileSync } from 'node:fs';
import { createTestServer } from './test-server.mjs';

globalThis.location = new URL('http://localhost/');
globalThis.window = new EventTarget();
const server = await createTestServer();
const { neuralView, pollsNeural, NOTICE, ABOUT } = await server.ssrLoadModule('/src/lib/neural-view.ts');
const { render } = await server.ssrLoadModule('svelte/server');
const NeuralRendering = (await server.ssrLoadModule('/src/components/library/NeuralRendering.svelte')).default;
await server.close();

/** `GET /api/neural` as the host answers it: an RX 7900 XT, the notice accepted, the switch off. */
const status = (overrides = {}) => ({
  accepted: true,
  available: true,
  gpu: 'AMD Radeon RX 7900 XT',
  reason: null,
  found: true,
  enabled: false,
  exe: null,
  release: null,
  job: null,
  ...overrides,
});
const job = (fields) => ({ action: 'install', phase: 'download', done: 0, total: 0, choose_exe: false, kept: [], ...fields });

test('an older GPU gets the switch disabled with the plain reason', () => {
  const reason = 'Neural Rendering runs on RDNA 3 and RDNA 4 Radeon graphics; this PC\'s AMD Radeon RX 6800 XT (gfx1030) is older';
  const view = neuralView(status({ available: false, gpu: null, reason }));
  assert.deepEqual(view, { kind: 'unavailable', reason: `${reason}.` });
  const html = render(NeuralRendering, { props: { uuid: 'game', initial: status({ available: false, reason }) } }).body;
  assert.match(html, /disabled/);
  assert.match(html, /RX 6800 XT/);
});

test('the download, the install and the removal show their progress and poll', () => {
  const download = neuralView(status({ job: job({ done: 504_000_000, total: 1_008_053_921 }) }));
  assert.equal(download.kind, 'working');
  assert.equal(download.percent, 50);
  assert.match(download.label, /504\u00a0MB of 1008\u00a0MB/);
  assert.ok(pollsNeural(download));
  assert.equal(neuralView(status({ job: job({}) })).percent, null);
  assert.match(neuralView(status({ job: job({ phase: 'place', done: 1, total: 4 }) })).label, /Installing into the game/);
  const removing = neuralView(status({ enabled: true, job: job({ action: 'remove', phase: 'remove' }) }));
  assert.equal(removing.percent, null);
  assert.ok(!pollsNeural(neuralView(status())));
});

test('the switch shows where it is, what stayed and why it failed', () => {
  assert.deepEqual(neuralView(status({ enabled: true, exe: 'C:\\Games\\Game.exe', release: 'Alpha0.3.5.8', job: job({ phase: 'done' }) })), {
    kind: 'on',
    exe: 'C:\\Games\\Game.exe',
    release: 'Alpha0.3.5.8',
  });
  // Still on even if this PC can no longer run it, so it can be turned off.
  assert.equal(neuralView(status({ enabled: true, available: false, reason: 'x' })).kind, 'on');
  assert.deepEqual(neuralView(status({ job: job({ action: 'remove', phase: 'done', kept: ['LmxxfNrRuntime.dll'] }) })), {
    kind: 'off',
    kept: ['LmxxfNrRuntime.dll'],
  });
  const refused = neuralView(status({ job: job({ phase: 'failed', error: 'the game uses Easy Anti-Cheat (C:\\Games\\EasyAntiCheat)' }) }));
  assert.deepEqual(refused, { kind: 'failed', error: 'the game uses Easy Anti-Cheat (C:\\Games\\EasyAntiCheat).', on: false });
  const choose = neuralView(status({ job: job({ phase: 'failed', choose_exe: true, error: 'Rubylight can\'t tell which program in C:\\Games\\X is the game' }) }));
  assert.equal(choose.kind, 'choose');
  const html = render(NeuralRendering, { props: { uuid: 'game', initial: status({ job: job({ phase: 'failed', choose_exe: true, error: 'which' }) }) } }).body;
  assert.match(html, /Choose the game’s \.exe/);
});

test('the notice says what it is, where it comes from and who does not stand behind it; no brand of the mod', () => {
  for (const words of [/third-party mod/, /authors’ GitHub release/, /not made or endorsed by AMD or NVIDIA/, /anti-cheat/, /own risk/]) {
    assert.match(NOTICE.message, words);
  }
  assert.notEqual(NOTICE.confirm, 'Continue');
  // GPL-3.0 §7(e) in the mod's license: its name is not ours to brand with. Credits live in NOTICE and AGENTS.md.
  const shown = [NOTICE.title, NOTICE.message, NOTICE.confirm, ABOUT].join('\n') +
    readFileSync(new URL('./src/components/library/NeuralRendering.svelte', import.meta.url), 'utf8') +
    readFileSync(new URL('./src/lib/neural-view.ts', import.meta.url), 'utf8');
  assert.doesNotMatch(shown, /AMDNR|AMD-NR|3zwr1|OptiScaler/i);
});
