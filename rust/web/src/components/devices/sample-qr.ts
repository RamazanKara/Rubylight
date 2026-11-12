// A picture of a QR code that encodes nothing: shown faded while pairing is off, so the
// code's place keeps its shape until one click allows pairing and a real code replaces it.
const SIZE = 25;

function sample(): string {
  const dark = new Set<string>();
  const finder = (x: number, y: number) => {
    for (let dy = 0; dy < 7; dy++) {
      for (let dx = 0; dx < 7; dx++) {
        const ring = dx === 0 || dy === 0 || dx === 6 || dy === 6;
        const core = dx >= 2 && dx <= 4 && dy >= 2 && dy <= 4;
        if (ring || core) dark.add(`${x + dx},${y + dy}`);
      }
    }
  };
  const inFinder = (x: number, y: number) =>
    (x < 8 && y < 8) || (x >= SIZE - 8 && y < 8) || (x < 8 && y >= SIZE - 8);
  finder(0, 0);
  finder(SIZE - 7, 0);
  finder(0, SIZE - 7);
  // A fixed pseudo-random fill, the same on every load.
  let seed = 0x2f6b;
  for (let y = 0; y < SIZE; y++) {
    for (let x = 0; x < SIZE; x++) {
      seed = (Math.imul(seed, 1103515245) + 12345) & 0x7fffffff;
      if (!inFinder(x, y) && (seed >>> 16) % 100 < 46) dark.add(`${x},${y}`);
    }
  }
  const cells = [...dark].map((cell) => {
    const [x, y] = cell.split(',');
    return `M${x} ${y}h1v1h-1z`;
  });
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="-1 -1 ${SIZE + 2} ${SIZE + 2}" shape-rendering="crispEdges"><rect x="-1" y="-1" width="${SIZE + 2}" height="${SIZE + 2}" fill="#fff"/><path d="${cells.join('')}" fill="#000"/></svg>`;
  return `data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`;
}

export const SAMPLE_QR = sample();
