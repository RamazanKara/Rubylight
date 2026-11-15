import assert from 'node:assert/strict';
import { test } from 'node:test';
import { readFileSync } from 'node:fs';
import { createTestServer } from './test-server.mjs';

globalThis.location = new URL('http://localhost/');
globalThis.window = new EventTarget();
const server = await createTestServer();
const { neuralView, pollsNeural, NOTICE, ABOUT, REPLACES } = await server.ssrLoadModule('/src/lib/neural-view.ts');
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
  mode: null,
  earlier_upscaling: null,
  job: null,
  ...overrides,
});
const job = (fields) => ({ action: 'install', phase: 'download', done: 0, total: 0, choose_exe: false, kept: [], ...fields });

test('an older GPU gets the switch disabled with the plain reason', () => {
  const reason = 'frame generation runs on AMD Radeon RX 5000 and newer graphics (RDNA) and NVIDIA GeForce GTX 16, RTX 20 and newer; this PC streams with AMD Radeon VII (gfx906), which is older';
  const view = neuralView(status({ available: false, gpu: null, reason }));
  assert.deepEqual(view, { kind: 'unavailable', reason: `${reason}.`, earlier: null });
  const html = render(NeuralRendering, { props: { uuid: 'game', initial: status({ available: false, reason }) } }).body;
  assert.match(html, /disabled/);
  assert.match(html, /Radeon VII/);
  assert.match(html, /Frame generation/);
});

test('the earlier neural upscaling is offered only for removal, also where frame generation is not', () => {
  const earlier_upscaling = { exe: 'C:\\Games\\Game.exe', release: 'Alpha0.3.5.8R2' };
  assert.deepEqual(neuralView(status({ earlier_upscaling })), { kind: 'off', kept: [], earlier: 'Alpha0.3.5.8R2' });
  const unavailable = neuralView(status({ available: false, reason: 'x', earlier_upscaling }));
  assert.equal(unavailable.earlier, 'Alpha0.3.5.8R2');
  const html = render(NeuralRendering, { props: { uuid: 'game', initial: status({ earlier_upscaling }) } }).body;
  assert.match(html, /earlier neural upscaling \(release Alpha0\.3\.5\.8R2\) is still in the game’s folder/);
  assert.match(html, />Remove</);
  assert.ok(html.includes(REPLACES));
  // Frame generation going in takes it out first, and says so.
  const replacing = neuralView(status({ earlier_upscaling, job: job({ phase: 'remove' }) }));
  assert.equal(replacing.label, 'Removing the earlier neural upscaling…');
  assert.doesNotMatch(render(NeuralRendering, { props: { uuid: 'game', initial: status() } }).body, />Remove</);
  // A failure that tells the user to remove it on the game's page (a refusal, a drive offline) and a
  // program to choose keep its Remove.
  const told = 'the earlier Neural Rendering upscaling is in the game\'s folder: remove it on the game\'s page first';
  for (const failed of [
    job({ phase: 'failed', error: told }),
    job({ action: 'remove', phase: 'failed', error: 'the game\'s folder can\'t be reached' }),
    job({ phase: 'failed', choose_exe: true, error: 'which' }),
  ]) {
    const view = neuralView(status({ earlier_upscaling, job: failed }));
    assert.equal(view.earlier, 'Alpha0.3.5.8R2', JSON.stringify(view));
    const page = render(NeuralRendering, { props: { uuid: 'game', initial: status({ earlier_upscaling, job: failed }) } }).body;
    assert.match(page, />Remove</, view.kind);
  }
});

test('the download, the install and the removal show their progress and poll', () => {
  const download = neuralView(status({ job: job({ done: 7_000_000, total: 13_939_157 }) }));
  assert.equal(download.kind, 'working');
  assert.equal(download.percent, 50);
  assert.match(download.label, /^Downloading frame generation: 7\u00a0MB of 14\u00a0MB/);
  assert.ok(pollsNeural(download));
  assert.equal(neuralView(status({ job: job({}) })).percent, null);
  assert.match(neuralView(status({ job: job({ phase: 'place', done: 1, total: 4 }) })).label, /Installing into the game/);
  const removing = neuralView(status({ enabled: true, job: job({ action: 'remove', phase: 'remove' }) }));
  assert.equal(removing.percent, null);
  assert.ok(!pollsNeural(neuralView(status())));
});

test('the switch shows where it is, what stayed and why it failed', () => {
  assert.deepEqual(neuralView(status({ enabled: true, exe: 'C:\\Games\\Game.exe', release: 'FG-V.B16', mode: 'full', job: job({ phase: 'done' }) })), {
    kind: 'on',
    exe: 'C:\\Games\\Game.exe',
    release: 'FG-V.B16',
    addon: false,
  });
  const added = { enabled: true, exe: 'C:\\Games\\Game.exe', release: 'FG-V.B16', mode: 'addon' };
  assert.equal(neuralView(status(added)).addon, true);
  assert.match(render(NeuralRendering, { props: { uuid: 'game', initial: status(added) } }).body, /Added to the upscaler mod beside/);
  // Still on even if this PC can no longer run it, so it can be turned off.
  assert.equal(neuralView(status({ enabled: true, available: false, reason: 'x' })).kind, 'on');
  assert.deepEqual(neuralView(status({ job: job({ action: 'remove', phase: 'done', kept: ['nvngx_dlssg.dll'] }) })), {
    kind: 'off',
    kept: ['nvngx_dlssg.dll'],
    earlier: null,
  });
  const refused = neuralView(status({ job: job({ phase: 'failed', error: 'the game uses Easy Anti-Cheat (C:\\Games\\EasyAntiCheat)' }) }));
  assert.deepEqual(refused, {
    kind: 'failed',
    title: 'Frame generation couldn’t be installed',
    error: 'the game uses Easy Anti-Cheat (C:\\Games\\EasyAntiCheat).',
    on: false,
    earlier: null,
  });
  // A launch's check that failed removed nothing, and a removal that failed leaves the switch on.
  const checked = neuralView(status({ enabled: true, job: job({ action: 'update', phase: 'failed', error: 'a file changed' }) }));
  assert.deepEqual(checked, { kind: 'failed', title: 'Frame generation couldn’t be checked for the launch', error: 'a file changed.', on: true, earlier: null });
  assert.equal(
    neuralView(status({ enabled: true, job: job({ action: 'remove', phase: 'failed', error: 'in use' }) })).title,
    'Neural Rendering couldn’t be removed',
  );
  const choose = neuralView(status({ job: job({ phase: 'failed', choose_exe: true, error: 'Rubylight can\'t tell which program in C:\\Games\\X is the game' }) }));
  assert.equal(choose.kind, 'choose');
  const html = render(NeuralRendering, { props: { uuid: 'game', initial: status({ job: job({ phase: 'failed', choose_exe: true, error: 'which' }) }) } }).body;
  assert.match(html, /Choose the game’s \.exe/);
});

test('the notice says what it is, where it comes from and who does not stand behind it; no brand of the mod', () => {
  for (const words of [/third-party mod/, /DLSS frame generation/, /authors’ GitHub release/, /not made or endorsed by AMD or NVIDIA/, /anti-cheat/, /own risk/]) {
    assert.match(NOTICE.message, words);
  }
  assert.notEqual(NOTICE.confirm, 'Continue');
  // GPL-3.0 §7(e) in the mods' licenses: their names are not ours to brand with. Credits live in NOTICE and AGENTS.md.
  const shown = [NOTICE.title, NOTICE.message, NOTICE.confirm, ABOUT].join('\n') +
    readFileSync(new URL('./src/components/library/NeuralRendering.svelte', import.meta.url), 'utf8') +
    readFileSync(new URL('./src/lib/neural-view.ts', import.meta.url), 'utf8');
  assert.doesNotMatch(shown, /AMDNR|AMD-NR|3zwr1|OptiScaler|\bANR\b|Apex|experimental|\bbeta\b/i);
});

test('the text claims nothing the host neither checks nor measured', () => {
  for (const text of [ABOUT, ...Object.values(NOTICE).flat()].filter((t) => typeof t === 'string')) {
    assert.doesNotMatch(text, /video memory|VRAM|\bGB of\b/i, text);
  }
});
