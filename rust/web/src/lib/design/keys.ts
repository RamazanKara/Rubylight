// Keyboard models shared by composite widgets (tabs, segmented controls, grids).

/** Orientation of a one-dimensional composite widget. */
export type Orientation = 'horizontal' | 'vertical';

/**
 * The index a roving-tabindex widget moves to for a key press, or null when the
 * key is not one it handles. Arrows wrap; Home and End jump to the ends;
 * disabled items are skipped.
 */
export function rovingIndex(
  current: number,
  key: string,
  count: number,
  orientation: Orientation = 'horizontal',
  disabled: (index: number) => boolean = () => false,
): number | null {
  if (count <= 0) return null;
  const [back, forward] = orientation === 'horizontal' ? ['ArrowLeft', 'ArrowRight'] : ['ArrowUp', 'ArrowDown'];
  let step: number;
  let from: number;
  if (key === forward) [step, from] = [1, current];
  else if (key === back) [step, from] = [-1, current];
  else if (key === 'Home') [step, from] = [1, -1];
  else if (key === 'End') [step, from] = [-1, count];
  else return null;
  for (let i = 1; i <= count; i++) {
    const next = (((from + step * i) % count) + count) % count;
    if (!disabled(next)) return next;
  }
  return null;
}
