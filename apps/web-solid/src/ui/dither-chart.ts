/** Settings usage-chart raster. Keep self-contained: inline previews serialize this into their isolated frame. */
export function paintDitherAreas(
  ctx: CanvasRenderingContext2D,
  width: number,
  height: number,
  max: number,
  values: readonly (readonly number[])[],
  colors: readonly (readonly number[])[],
  alternateDotted = true,
): void {
  const bayer = [0, 8, 2, 10, 12, 4, 14, 6, 3, 11, 1, 9, 15, 7, 13, 5];
  if (!values.length || !colors.length || width <= 0 || height <= 0 || max <= 0) return;
  const cols = Math.min(520, Math.max(8, Math.round(width / 2)));
  const rows = Math.min(200, Math.max(8, Math.round(height / 2)));
  const cw = width / cols, ch = height / rows;
  for (let series = 0; series < (values[0]?.length ?? 0); series++) {
    const dotted = alternateDotted && series % 2 === 1;
    const [r, g, b] = colors[series % colors.length]!;
    for (let x = 0; x < cols; x++) {
      const position = (x / (cols - 1)) * Math.max(0, values.length - 1);
      const a = Math.min(Math.floor(position), values.length - 1);
      const next = Math.min(a + 1, values.length - 1);
      const t = position - a;
      const value = (values[a]?.[series] ?? 0) * (1 - t) + (values[next]?.[series] ?? 0) * t;
      const start = Math.min(rows, Math.max(0, Math.round((1 - value / max) * rows)));
      const depth = rows - start;
      for (let y = start; y < rows; y++) {
        let alpha: number;
        if (y === start) alpha = 0.72;
        else if (y === start + 1) alpha = 0.36;
        else {
          const density = (y - start) / Math.max(1, depth);
          const bias = (dotted ? 0.12 : 0) - series * 0.14;
          const lit = density > (bayer[(y & 3) * 4 + (x & 3)]! + 0.5) / 16 - bias;
          if (dotted && !lit) continue;
          alpha = (0.3 + density * 0.7) * (lit ? 1 : 0.4);
        }
        ctx.fillStyle = `rgba(${r},${g},${b},${alpha})`;
        ctx.fillRect(x * cw, y * ch, cw + 0.01, ch + 0.01);
      }
    }
  }
}

/** Viewport dither_columns.rs: dense floor, dissolving body, solid cap on a 2px cell grid. */
export function paintDitherColumns(
  ctx: CanvasRenderingContext2D,
  width: number,
  height: number,
  max: number,
  values: readonly number[],
  color: readonly number[],
): void {
  const bayer = [0, 8, 2, 10, 12, 4, 14, 6, 3, 11, 1, 9, 15, 7, 13, 5];
  if (!values.length || width <= 0 || height <= 0 || max <= 0) return;
  const band = width / values.length, rows = Math.floor(height / 2);
  const [red, green, blue] = color;
  for (let i = 0; i < values.length; i++) {
    const columnWidth = Math.floor(band * 0.72 / 2) * 2;
    const left = Math.round((i * band + (band - columnWidth) / 2) / 2) * 2;
    const lit = Math.ceil(Math.min(1, Math.max(0, values[i]! / max)) * rows);
    for (let x = 0; x < columnWidth / 2; x++) for (let row = 0; row < lit; row++) {
      const cap = row + 1 === lit;
      const threshold = (bayer[(row & 3) * 4 + (x & 3)]! + 0.5) / 16;
      const on = 0.3 + 0.7 * (1 - row / lit) > threshold;
      ctx.fillStyle = `rgba(${red},${green},${blue},${cap || on ? 1 : 0.22})`;
      ctx.fillRect(left + x * 2, height - (row + 1) * 2, 2, 2);
    }
  }
}
