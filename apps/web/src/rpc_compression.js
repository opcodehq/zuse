// Optional direct-link compression uses the native browser codec, no WASM codec.
export function supportsRpcCompression() {
  try { new DecompressionStream("deflate"); return true; } catch { return false; }
}

export function createRpcDecoder({timeoutMs = 15000} = {}) {
  let reader, writer, failed = false;
  const close = () => {
    failed = true;
    reader?.cancel().catch(() => {});
    writer?.abort().catch(() => {});
  };
  return {
    close,
    async decode(bytes, maxBytes) {
      if (failed) throw new Error("Engine compression stream is closed");
      if (bytes[0] !== 0x5a || bytes[1] !== 0x44 || bytes.length < 7)
        throw new Error("Unexpected compressed engine frame");
      const expected = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength).getUint32(2, true);
      if (!expected || expected > maxBytes) {
        close(); throw new Error("Decoded engine frame exceeds the message limit");
      }
      if (!reader) {
        const stream = new DecompressionStream("deflate");
        reader = stream.readable.getReader(); writer = stream.writable.getWriter();
      }
      // Drain while writing: native TransformStream backpressure otherwise
      // deadlocks on a large snapshot. Calls are serialized by the socket owner.
      const write = writer.write(bytes.subarray(6));
      write.catch(() => {});
      const decoder = new TextDecoder("utf-8", {fatal:true}), chunks = [];
      let total = 0;
      // All compressed bytes are already local. A truncated streaming frame
      // must not suspend receive forever while waiting for a future packet.
      const deadline = setTimeout(close, timeoutMs);
      try {
        while (total < expected) {
          const {value, done} = await reader.read();
          if (done) throw new Error("Truncated engine compression stream");
          total += value.byteLength;
          if (total > expected) throw new Error("Decoded engine frame exceeds the message limit");
          chunks.push(decoder.decode(value, {stream:true}));
        }
        await write;
        chunks.push(decoder.decode());
        return chunks.join("");
      } catch (error) { close(); throw error; }
      finally { clearTimeout(deadline); }
    },
  };
}

export async function decodeRpcFrame(bytes, maxBytes, streamDecoder) {
  if (bytes[0] === 0x5a && bytes[1] === 0x44) {
    if (!streamDecoder) throw new Error("Missing engine compression stream");
    return streamDecoder.decode(bytes, maxBytes);
  }
  // Compatibility with gzip-v1 hosts and fixture clients.
  if (bytes[0] !== 0x1f || bytes[1] !== 0x8b)
    throw new Error("Unexpected compressed engine frame");
  const reader = new Blob([bytes]).stream()
    .pipeThrough(new DecompressionStream("gzip")).getReader();
  const decoder = new TextDecoder("utf-8", {fatal:true}), chunks = [];
  let total = 0;
  try {
    for (;;) {
      const {value, done} = await reader.read();
      if (done) break;
      total += value.byteLength;
      if (total > maxBytes) {
        await reader.cancel();
        throw new Error("Decoded engine frame exceeds the message limit");
      }
      chunks.push(decoder.decode(value, {stream:true}));
    }
    chunks.push(decoder.decode());
    return chunks.join("");
  } finally { reader.releaseLock(); }
}
