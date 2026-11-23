// crates/viewport/src/dither.rs::ramp: 2px Bayer cells, increasing density
// toward the knob, with 18% ink in the unlit cells and rounded-cell clipping.
const BAYER = [0, 8, 2, 10, 12, 4, 14, 6, 3, 11, 1, 9, 15, 7, 13, 5];
export function reasoningDither(width: number) {
  const columns = Math.max(1, Math.round(width / 2));
  const cw = width / columns;
  const cells: { x: number; y: number; width: number; opacity: number }[] = [];
  for (let y = 0; y < 10; y++) for (let x = 0; x < columns; x++) {
    const cx = (x + .5) * cw, cy = (y + .5) * 2;
    const dx = Math.max(10 - cx, cx - (width - 10), 0);
    const dy = Math.max(10 - cy, cy - 10, 0);
    if (dx * dx + dy * dy > 100) continue;
    const density = .15 + .85 * Math.pow((x + 1) / columns, 1.4);
    const threshold = BAYER[(y & 3) * 4 + (x & 3)]! / 16 + .5 / 16;
    cells.push({ x: x * cw, y: y * 2, width: cw, opacity: density > threshold ? 1 : .18 });
  }
  return cells;
}
