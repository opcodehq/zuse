// Image bytes from a computer or an agent become a URL an <img> can show without handing active content our origin.
// A blob: URL runs as this origin when opened as a page ("Open image in new tab"), so only raster types get one.
// SVG can carry script; it becomes a data: URL, which browsers refuse to open as a page and which has an opaque
// origin if it ever is. Anything else is labelled octet-stream so it can only download.

const RASTER = new Set(["image/png", "image/jpeg", "image/gif", "image/webp", "image/avif", "image/bmp", "image/x-icon", "image/vnd.microsoft.icon"]);
const SVG = "image/svg+xml";
const SVG_MAX = 8 * 1024 * 1024;

const BY_EXTENSION: Record<string, string> = {
  png: "image/png", jpg: "image/jpeg", jpeg: "image/jpeg", gif: "image/gif", webp: "image/webp",
  avif: "image/avif", bmp: "image/bmp", ico: "image/x-icon", svg: SVG,
};

/** The MIME type to trust for an image: a reported type when it is one we know, else the file extension. */
export function imageMime(path: string, reported?: string | null): string {
  const type = reported?.split(";")[0]?.trim().toLowerCase() ?? "";
  if (RASTER.has(type) || type === SVG) return type;
  return BY_EXTENSION[path.split(".").pop()?.toLowerCase() ?? ""] ?? "application/octet-stream";
}

export const isRasterMime = (mime: string) => RASTER.has(mime);

/** A displayable URL for image bytes. Revoke it with `releaseImageUrl`. */
export async function imageUrl(parts: BlobPart[], mime: string): Promise<string> {
  if (RASTER.has(mime)) return URL.createObjectURL(new Blob(parts, { type: mime }));
  const blob = new Blob(parts, { type: mime === SVG ? SVG : "application/octet-stream" });
  if (mime !== SVG) return URL.createObjectURL(blob);
  if (blob.size > SVG_MAX) throw new Error("This SVG is too large to preview here.");
  return await new Promise<string>((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result));
    reader.onerror = () => reject(reader.error ?? new Error("Couldn't read this image."));
    reader.readAsDataURL(blob);
  });
}

/** A preview URL for a local file the user picked: raster files only. */
export function filePreviewUrl(file: File): string | undefined {
  return RASTER.has(file.type.toLowerCase()) ? URL.createObjectURL(file) : undefined;
}

export function releaseImageUrl(url: string | null | undefined) {
  if (url?.startsWith("blob:")) URL.revokeObjectURL(url);
}
