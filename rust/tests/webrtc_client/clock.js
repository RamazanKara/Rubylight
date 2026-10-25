// The glass-to-glass time code shared by the clock page (writer) and the
// probe (reader). A magenta block holds CELLS black and white cells: two
// guard cells (1, 0), 24 bits of the millisecond clock and two more guards
// (0, 1). The reader finds the block by its magenta border, so the window can
// sit anywhere on the captured display at any stream scale.
(function (root) {
  const layout = {CELLS: 28, BITS: 24, CELL_W: 48, CELL_H: 96, PAD: 40};
  layout.WIDTH = layout.CELLS * layout.CELL_W + 2 * layout.PAD;
  layout.HEIGHT = layout.CELL_H + 2 * layout.PAD;
  // The cell values for the low 24 bits of `ms`.
  function cells(ms) {
    const value = ms % (1 << layout.BITS);
    const bits = [];
    for (let i = layout.BITS - 1; i >= 0; i--) bits.push((value >> i) & 1);
    return [1, 0, ...bits, 0, 1];
  }
  // The time code, or null when the guard cells are wrong.
  function decode(bits) {
    if (bits.length !== layout.CELLS || bits[0] !== 1 || bits[1] !== 0 || bits[layout.CELLS - 2] !== 0 || bits[layout.CELLS - 1] !== 1) return null;
    let value = 0;
    for (let i = 2; i < layout.CELLS - 2; i++) value = value * 2 + bits[i];
    return value;
  }
  const api = {layout, cells, decode};
  if (typeof module !== 'undefined' && module.exports) module.exports = api; else root.RlClock = api;
})(typeof globalThis !== 'undefined' ? globalThis : this);
