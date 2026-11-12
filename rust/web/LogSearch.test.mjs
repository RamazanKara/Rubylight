import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

const server = await createTestServer();
const { MAX_MARKS, countMatches, indexOfId, markedRange, parse, pieces, searchPattern } = await server.ssrLoadModule(
  '/src/components/logs/logtext.ts',
);

test('a line is cut at its matches, the timestamp apart', () => {
  const [line] = parse(['2026-11-12T16:00:10.1Z  INFO stream: frame 16 of 16'], null);
  const cut = pieces(line, searchPattern('16'));
  assert.deepEqual(
    cut.filter((piece) => piece.hit >= 0).map((piece) => [piece.text, piece.time, piece.hit]),
    [['16', true, 0], ['16', false, 1], ['16', false, 2]],
  );
  assert.equal(cut.map((piece) => piece.text).join(''), line.text);
});

test('a line of thousands of matches marks only those around the current one', () => {
  const [line] = parse(['e'.repeat(64 * 1024)], null);
  const pattern = searchPattern('e');
  assert.equal(countMatches(line.text, pattern), 64 * 1024);
  const hits = (around) => pieces(line, pattern, around).filter((piece) => piece.hit >= 0).map((piece) => piece.hit);
  const first = hits(0);
  assert.equal(first.length, MAX_MARKS);
  assert.deepEqual([first[0], first.at(-1)], [0, MAX_MARKS - 1]);
  // Stepping to match 30000 still finds it marked, and the text stays whole.
  const far = hits(30000);
  assert.equal(far.length, MAX_MARKS);
  assert.ok(far.includes(30000));
  assert.equal(pieces(line, pattern, 30000).map((piece) => piece.text).join(''), line.text);
});

test('matches are marked around the line in view, not in all 5000 lines', () => {
  const lines = parse(Array.from({ length: 5000 }, (_, i) => `line ${i}`), null);
  const id = (index) => lines[index].id;
  assert.equal(indexOfId(lines, id(0)), 0);
  assert.equal(indexOfId(lines, id(4999)), 4999);
  assert.equal(indexOfId(lines, id(1234)), 1234);
  assert.deepEqual(markedRange(lines, id(2500), 200), { from: id(2300), to: id(2700) });
  assert.deepEqual(markedRange(lines, id(10), 200), { from: id(0), to: id(210) });
  assert.deepEqual(markedRange(lines, id(4990), 200), { from: id(4790), to: id(4999) });
  // A level filter leaves gaps in the ids, and old lines drop off the top: the nearest line counts.
  const errors = lines.filter((_, index) => index % 97 === 0);
  assert.equal(indexOfId(errors, id(98)), 2);
  assert.equal(indexOfId(errors, 0), 0);
  assert.equal(indexOfId(errors, id(4999) + 50), errors.length - 1);
  assert.deepEqual(markedRange(errors, id(970), 3), { from: id(7 * 97), to: id(13 * 97) });
  assert.deepEqual(markedRange([], 5, 200), { from: 0, to: -1 });
});

test.after(() => server.close());
