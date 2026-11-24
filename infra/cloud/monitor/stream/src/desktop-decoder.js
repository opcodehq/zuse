function createDesktopPainter(ctx) {
  // Patches carry their decoded size. Its high bit selects DEFLATE.
  let desktop;
  const scratch = new Uint8ClampedArray(1024 * 1024);
  async function unpackPatch(src) {
    const lengthWord = new DataView(
      src.buffer,
      src.byteOffset,
      src.byteLength,
    ).getUint32(0, true);
    const size = lengthWord & 0x7fffffff;
    if (size < 2 || size > scratch.length) throw Error("Invalid patch size");
    if (lengthWord >>> 31) {
      const reader = new Blob([src.subarray(4)])
        .stream()
        .pipeThrough(new DecompressionStream("deflate"))
        .getReader();
      let offset = 0;
      for (;;) {
        const { done, value } = await reader.read();
        if (done) break;
        if (offset + value.length > size) {
          await reader.cancel();
          throw Error("Oversized patch");
        }
        scratch.set(value, offset);
        offset += value.length;
      }
      if (offset !== size) throw Error("Truncated patch");
      return scratch.subarray(0, size);
    }
    let read = 4,
      written = 0;
    function length(n) {
      if (n === 15) {
        let extra;
        do {
          if (read >= src.length) throw Error("Truncated patch");
          extra = src[read++];
          n += extra;
        } while (extra === 255);
      }
      return n;
    }
    while (read < src.length) {
      const token = src[read++];
      const literals = length(token >> 4);
      if (read + literals > src.length || written + literals > size)
        throw Error("Invalid literal length");
      scratch.set(src.subarray(read, read + literals), written);
      read += literals;
      written += literals;
      if (read === src.length) break;
      if (read + 2 > src.length) throw Error("Missing match");
      const distance = src[read++] | (src[read++] << 8);
      let count = 4 + length(token & 15);
      if (!distance || distance > written || written + count > size)
        throw Error("Invalid match");
      let available = distance;
      while (count > 0) {
        const n = Math.min(count, available);
        scratch.copyWithin(
          written,
          written - available,
          written - available + n,
        );
        written += n;
        count -= n;
        available += n;
      }
    }
    if (written !== size) throw Error("Incomplete patch");
    return scratch.subarray(0, size);
  }
  async function paintIndexed(encoded) {
    const header = new DataView(
      encoded.buffer,
      encoded.byteOffset,
      encoded.byteLength,
    );
    const w = header.getUint16(0, true),
      h = header.getUint16(2, true),
      format = header.getUint16(4, true),
      n = format & 0x7fff;
    if (
      !((w === 1920 && h === 1080) || (w === 2560 && h === 1600)) ||
      !n ||
      n > 256
    )
      throw Error("Invalid indexed desktop");
    const palette = new Uint32Array(n);
    for (let i = 0; i < n; i++) palette[i] = header.getUint32(6 + i * 4, true);
    const reader = new Blob([encoded.subarray(6 + n * 4)])
      .stream()
      .pipeThrough(new DecompressionStream("deflate"))
      .getReader();
    const img = new ImageData(w, h),
      pixels = new Uint32Array(img.data.buffer);
    const dictionary = (format & 0x8000) !== 0;
    const data = new Uint8Array(dictionary ? 1024 * 1024 : w * h);
    let offset = 0;
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      if (offset + value.length > data.length) {
        await reader.cancel();
        throw Error("Oversized desktop");
      }
      data.set(value, offset);
      offset += value.length;
    }
    const start = dictionary ? 2 * h : 0;
    const count = offset - start;
    if (count <= 0 || count % w || (!dictionary && count !== pixels.length))
      throw Error("Invalid desktop rows");
    const colors = dictionary ? new Uint32Array(count) : pixels;
    for (let i = 0; i < count; i++) {
      const id = data[start + i];
      if (id >= n) throw Error("Invalid palette index");
      colors[i] = palette[id];
    }
    if (dictionary) {
      const refs = new DataView(data.buffer, 0, start);
      for (let y = 0; y < h; y++) {
        const row = refs.getUint16(y * 2, true) * w;
        if (row + w > count) throw Error("Invalid desktop row");
        pixels.set(colors.subarray(row, row + w), y * w);
      }
    }
    if (ctx.canvas.width !== w || ctx.canvas.height !== h) {
      ctx.canvas.width = w;
      ctx.canvas.height = h;
    }
    ctx.putImageData(img, 0, 0);
    desktop = img;
    return true;
  }
  async function paintPatch(encoded) {
    const raw = await unpackPatch(encoded);
    const view = new DataView(raw.buffer, raw.byteOffset, raw.byteLength);
    let shift = view.getInt16(0, true);
    if (shift === 0 && raw.length === 2) return false;
    let x = 0,
      y = 0,
      w = ctx.canvas.width,
      h = ctx.canvas.height,
      offset = 2;
    if (shift === -32768) {
      shift = view.getInt16(2, true);
      x = view.getUint16(4, true);
      y = view.getUint16(6, true);
      w = view.getUint16(8, true);
      h = view.getUint16(10, true);
      if (!w || !h || x + w > ctx.canvas.width || y + h > ctx.canvas.height)
        throw Error("Invalid scroll rectangle");
      offset = 12;
    }
    if (Math.abs(shift) > 32) throw Error("Invalid scroll offset");
    const overlap = h - Math.abs(shift);
    const stride = ctx.canvas.width * 4;
    desktop ??= ctx.getImageData(0, 0, ctx.canvas.width, ctx.canvas.height);
    const pixels = desktop.data;
    if (shift && overlap > 0) {
      const start = (y + Math.max(shift, 0)) * stride + x * 4;
      const dest = (y + Math.max(-shift, 0)) * stride + x * 4;
      if (w === ctx.canvas.width) {
        pixels.copyWithin(dest, start, start + overlap * stride);
      } else {
        for (let i = 0; i < overlap; i++) {
          const offset = (shift > 0 ? i : overlap - 1 - i) * stride;
          pixels.copyWithin(
            dest + offset,
            start + offset,
            start + offset + w * 4,
          );
        }
      }
    }
    while (offset < raw.length) {
      const left = view.getUint16(offset, true),
        top = view.getUint16(offset + 2, true);
      const width = view.getUint16(offset + 4, true);
      offset += 6;
      const end = offset + width * 4;
      if (
        end > raw.length ||
        !width ||
        left < x ||
        left + width > x + w ||
        top < y ||
        top >= y + h
      )
        throw Error("Invalid patch extent");
      pixels.set(raw.subarray(offset, end), top * stride + left * 4);
      offset = end;
    }
    ctx.putImageData(desktop, 0, 0, x, y, w, h);
    return true;
  }
  return {
    paintIndexed,
    paintPatch,
    invalidate: () => {
      desktop = undefined;
    },
  };
}
