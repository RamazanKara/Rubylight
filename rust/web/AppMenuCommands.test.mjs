import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

const server = await createTestServer();
const { AppDraft } = await server.ssrLoadModule('/src/components/library/draft.svelte.ts');
const { menuRow, MENU_LIMIT } = await server.ssrLoadModule('/src/components/library/menucommands.ts');
const app = extra => ({ name: 'Game', cmd: 'game.exe', 'working-dir': '', 'prep-cmd': [], uuid: 'a', ...extra });

test('an app without menu commands sends none, and a blank row changes nothing', () => {
  const draft = new AppDraft(app({}));
  assert.ok(!('menu-cmd' in draft.record()));
  draft.menu.push(menuRow());
  assert.equal(draft.changed, false);
  assert.equal(draft.invalid, false);
});

test('stored commands read back unchanged, keep a Foundation id, and an emptied list is removed', () => {
  const stored = [
    { id: 'pet', name: 'Desktop pet', cmd: 'pet.exe', undo: 'taskkill /im pet.exe', elevated: false },
    { name: 'Keyboard', cmd: 'osk', elevated: true },
  ];
  const draft = new AppDraft(app({ 'menu-cmd': stored }));
  assert.equal(draft.changed, false);
  assert.deepEqual(draft.record()['menu-cmd'], stored);
  draft.menu.push(menuRow({ name: ' Lights ', cmd: 'lights on', undo: 'lights off' }));
  assert.equal(draft.changed, true);
  assert.deepEqual(draft.record()['menu-cmd'][2], { name: 'Lights', cmd: 'lights on', undo: 'lights off', elevated: false });
  draft.menu.splice(0, 3);
  assert.ok(!('menu-cmd' in draft.record()));
});

test('a command without a name or a command line cannot be saved', () => {
  for (const row of [{ name: 'Pet' }, { cmd: 'pet.exe' }, { undo: 'taskkill /im pet.exe' }]) {
    const draft = new AppDraft(app({}));
    draft.menu.push(menuRow(row));
    assert.equal(draft.invalid, true, JSON.stringify(row));
    assert.notEqual(draft.menuError, '');
  }
  const draft = new AppDraft(app({}));
  for (let i = 0; i <= MENU_LIMIT; i++) draft.menu.push(menuRow({ name: `C${i}`, cmd: 'x' }));
  assert.match(draft.menuError, /at most/);
});

test('hiding the host-wide PC commands is an app switch, off by default', () => {
  const draft = new AppDraft(app({}));
  assert.equal(draft.flag('exclude-global-menu-cmd'), false);
  draft.setFlag('exclude-global-menu-cmd', true);
  assert.equal(draft.record()['exclude-global-menu-cmd'], true);
});
