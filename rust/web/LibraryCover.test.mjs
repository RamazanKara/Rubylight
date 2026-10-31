import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

const server = await createTestServer();
const { placeholder } = await server.ssrLoadModule('/src/components/library/app.ts');
const grid = readFileSync(new URL('./src/components/library/LibraryGrid.svelte', import.meta.url), 'utf8');
const sources = readFileSync(new URL('./src/components/library/LauncherSources.svelte', import.meta.url), 'utf8');

test('a cover without art keeps its tint per game and draws the Desktop and Big Picture as pictures', () => {
  const portal = placeholder('Portal 2');
  assert.deepEqual(placeholder(' portal 2 '), portal, 'the same game always gets the same tint');
  assert.equal(portal.letters, 'P2');
  // A sequel's roman numeral reads like a digit, after the first word only.
  assert.equal(placeholder('Hades II').letters, 'H2');
  assert.equal(placeholder('Diablo IV').letters, 'D4');
  assert.equal(placeholder('Ghost X').letters, 'G10');
  assert.equal(placeholder('Half-Life: Alyx').letters, 'HL');
  assert.equal(placeholder('X-Men Legends').letters, 'XM');
  assert.equal(placeholder('Grand Theft Auto V').letters, 'GT');
  assert.equal(portal.icon, null);
  assert.equal(placeholder('Desktop').icon, 'monitor');
  assert.equal(placeholder('Steam Big Picture').icon, 'gamepad');
  const hues = new Set(['Portal 2', 'Fortnite', 'Diablo IV', 'Hades II', 'Forza Horizon 5', 'Alan Wake 2'].map((name) => placeholder(name).hue));
  assert.ok(hues.size > 2, 'different games get different tints');
  for (const hue of hues) assert.ok(hue >= 0 && hue < 360);
});

test('each card names its one action for screen readers and keeps it reachable on touch', () => {
  assert.match(grid, /Launch \{app\.name\} on PC/);
  assert.match(grid, /Close \{app\.name\}/);
  assert.match(grid, /Remove \{app\.name\}/);
  // Hidden until hover only where there is a mouse; touch screens always show it.
  assert.match(grid, /@media \(hover: hover\) and \(pointer: fine\)\s*\{\s*\.reveal \{\s*opacity: 0;/);
  assert.match(grid, /\.card:focus-within \.reveal/);
});

test('a discovery reply without lists shows no launchers instead of breaking the Games page', () => {
  assert.match(sources, /sources: found\.sources \?\? \[\], games: found\.games \?\? \[\]/);
});
