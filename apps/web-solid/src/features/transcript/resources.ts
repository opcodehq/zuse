// Area-local data the engine serves on demand: full tool outputs and attachment images.
import { untrack } from "solid-js";
import { createStore } from "solid-js/store";
import { rpc } from "@/core/connection";
import { M } from "@/core/methods";
import { state } from "@/core/store";
import { imageMime, imageUrl, releaseImageUrl } from "@/ui/imageUrl";

// ---------------------------------------------------------------------------
// Fold and disclosure state, kept outside components so it survives virtualization.
// ---------------------------------------------------------------------------

const [open, setOpen] = createStore<Record<string, boolean>>({});
export const isOpen = (key: string, fallback = false) => open[key] ?? fallback;
export const setOpenKey = (key: string, value: boolean) => setOpen(key, value);
export const toggleOpen = (key: string, fallback = false) => setOpen(key, !isOpen(key, fallback));

// ---------------------------------------------------------------------------
// Full tool output (sidecar blobs)
// ---------------------------------------------------------------------------

export type BlobState = { status: "loading" } | { status: "ready"; text: string; total: number } | { status: "failed"; error: string };

const BLOB_PAGE = 65_536;
const BLOB_MAX = 512 * 1024;
const [blobs, setBlobs] = createStore<Record<string, BlobState>>({});
export const blobState = (ref: string) => blobs[ref];

interface BlobPage {
  text: string;
  nextOffset: number;
  totalBytes: number;
  done: boolean;
}

/** Fetch a tool's full output or diff by its doc-resident ref. Failed fetches retry on the next call. */
export async function fetchBlob(ref: string) {
  const current = untrack(() => blobs[ref]);
  if (current && current.status !== "failed") return;
  setBlobs(ref, { status: "loading" });
  // The ref is `{chatId}/{partId}`, so the owning chat names the device to ask.
  const owner = state.chats[ref.split("/")[0] ?? ""];
  try {
    let text = "";
    let offset = 0;
    let total = 0;
    for (;;) {
      const page = await rpc.call<BlobPage>(M.FETCH_TOOL_BLOB, {
        blobRef: ref,
        offset,
        limit: BLOB_PAGE,
        targetDeviceId: owner?.deviceId ?? null,
      });
      if (page.nextOffset < offset || page.nextOffset > page.totalBytes) throw new Error("Invalid tool output page");
      text += page.text;
      total = page.totalBytes;
      if (page.done || page.nextOffset === offset || page.nextOffset >= BLOB_MAX) break;
      offset = page.nextOffset;
    }
    setBlobs(ref, { status: "ready", text, total });
  } catch (error) {
    setBlobs(ref, { status: "failed", error: error instanceof Error ? error.message : String(error) });
  }
}

// ---------------------------------------------------------------------------
// Attachment images
// ---------------------------------------------------------------------------

export type ImageState = { status: "loading" } | { status: "ready"; url: string } | { status: "failed"; error: string };

const IMAGE_MAX = 8 * 1024 * 1024;
const IMAGE_CACHE = 64;
const [images, setImages] = createStore<Record<string, ImageState>>({});
const imageOrder: string[] = [];

interface AttachmentChunk {
  data: string;
  nextOffset: number;
  done: boolean;
}

const imageKey = (device: string | null, path: string) => `${device ?? ""}\u0000${path}`;
export const imageState = (device: string | null, path: string) => images[imageKey(device, path)];

/** Load an image from the device that owns it, in chunks, into an object URL. A failure stays until `retry`. */
export async function loadImage(device: string | null, path: string, mimeType?: string, retry = false) {
  const key = imageKey(device, path);
  const current = untrack(() => images[key]);
  if (current && (current.status !== "failed" || !retry)) return;
  setImages(key, { status: "loading" });
  try {
    const chunks: Uint8Array[] = [];
    let offset = 0;
    let size = 0;
    for (;;) {
      const chunk = await rpc.call<AttachmentChunk>(M.READ_ATTACHMENT_CHUNK, { path, offset, targetDeviceId: device });
      const binary = atob(chunk.data);
      const bytes = new Uint8Array(binary.length);
      for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
      if (chunk.nextOffset !== offset + bytes.length || (!chunk.done && !bytes.length)) throw new Error("Image read did not advance consistently.");
      size += bytes.length;
      if (size > IMAGE_MAX) throw new Error("Image is larger than the 8 MB preview limit.");
      chunks.push(bytes);
      offset = chunk.nextOffset;
      if (chunk.done) break;
    }
    // Never trust the part's type or the extension for a blob URL: `imageUrl` keeps SVG off this origin.
    const url = await imageUrl(chunks as BlobPart[], imageMime(path, mimeType));
    setImages(key, { status: "ready", url });
    imageOrder.push(key);
    while (imageOrder.length > IMAGE_CACHE) {
      const old = imageOrder.shift()!;
      const evicted = images[old];
      if (evicted?.status === "ready") releaseImageUrl(evicted.url);
      setImages(old, undefined as never);
    }
  } catch (error) {
    setImages(key, { status: "failed", error: error instanceof Error ? error.message : String(error) });
  }
}
