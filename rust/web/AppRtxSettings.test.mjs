import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

const server = await createTestServer();
const { render } = await server.ssrLoadModule('svelte/server');
const { AppDraft } = await server.ssrLoadModule('/src/components/library/draft.svelte.ts');
const RtxSettings = (await server.ssrLoadModule('/src/components/library/RtxSettings.svelte')).default;
const app = extra => ({ name: 'Game', cmd: 'game.exe', 'working-dir': '', 'prep-cmd': [], uuid: 'a', ...extra });
const body = (draft, offered) => render(RtxSettings, { props: { draft, offered } }).body;

test('an enhancement this PC offers shows its controls, a missing one says why', () => {
  const html = body(new AppDraft(app({})), {
    build: true,
    rtx_hdr: false,
    rtx_video: true,
    errors: { rtx_hdr: 'the NVIDIA driver is too old' },
  });
  assert.match(html, /RTX HDR is unavailable on this PC/);
  assert.match(html, /the NVIDIA driver is too old/);
  assert.doesNotMatch(html, /Use RTX HDR for this app/);
  assert.match(html, /Use the host setting/);
  assert.match(html, /Strength/);
  assert.doesNotMatch(html, /experimental|beta|preview/i);
});

test('RTX HDR is an opt-in, RTX Video follows the host until set, and both round-trip', () => {
  const draft = new AppDraft(app({}));
  assert.equal(draft.flag('rtx-hdr'), false);
  assert.equal(draft.tristate('rtx-video'), '');
  draft.setFlag('rtx-hdr', true);
  draft.setTristate('rtx-video', 'off');
  draft.app['rtx-video-intensity'] = 40;
  assert.equal(draft.numberError('rtx-video-intensity'), '');
  const record = draft.record();
  assert.equal(record['rtx-hdr'], true);
  assert.equal(record['rtx-video'], false);
  assert.equal(record['rtx-video-intensity'], 40);
  draft.app['rtx-video-intensity'] = 101;
  assert.notEqual(draft.numberError('rtx-video-intensity'), '');
  draft.setTristate('rtx-video', '');
  assert.ok(!('rtx-video' in draft.record()));
});
