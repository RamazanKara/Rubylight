import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

const server = await createTestServer();
const { placeholder } = await server.ssrLoadModule('/src/components/library/app.ts');
test('a cover without art keeps its tint per game and draws the Desktop and Big Picture as pictures', () => {
  const portal = placeholder('Portal 2');
  assert.deepEqual(placeholder(' portal 2 '), portal, 'the same game always gets the same tint');
  assert.equal(portal.letters, 'P2');
  assert.equal(portal.icon, null);
  assert.equal(placeholder('Desktop').icon, 'monitor');
  assert.equal(placeholder('Steam Big Picture').icon, 'gamepad');
  const hues = new Set(['Portal 2', 'Fortnite', 'Diablo IV', 'Hades II', 'Forza Horizon 5', 'Alan Wake 2'].map((name) => placeholder(name).hue));
  assert.ok(hues.size > 2, 'different games get different tints');
  for (const hue of hues) assert.ok(hue >= 0 && hue < 360);
});
