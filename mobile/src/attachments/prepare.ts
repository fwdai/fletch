// What a picked file becomes before it is uploaded. Most files go as they
// are; images that would be a poor use of the relay — or that the agent could
// not read at all — are re-encoded on the phone first.

/** Longest edge after re-encoding. Plenty for an agent reading a screenshot;
 *  a phone photo is 3–4× this and tens of megabytes. */
export const MAX_EDGE = 2048;

/** Images over this many bytes are re-encoded even when the format is fine. */
export const REENCODE_OVER_BYTES = 1.5 * 1024 * 1024;

/** JPEG quality for the re-encode. High enough that UI text in a screenshot
 *  stays legible; the size win over a raw PNG is still several-fold. */
const JPEG_QUALITY = 0.9;

export interface PreparedFile {
  name: string;
  bytes: Uint8Array;
}

const HEIC = /hei[cf]/i;

/** HEIC always: it is what the camera roll hands over and most agents cannot
 *  decode it. Otherwise only large raster images; GIFs and SVGs are left alone
 *  (a re-encode would drop animation or vectors). */
export function shouldReencode(file: { name: string; type: string; size: number }): boolean {
  const type = file.type.toLowerCase();
  if (HEIC.test(type) || HEIC.test(file.name)) return true;
  if (!type.startsWith("image/") || type === "image/gif" || type === "image/svg+xml") return false;
  return file.size > REENCODE_OVER_BYTES;
}

/** The name the re-encoded copy is staged under: the original stem, `.jpg`. */
export function jpegName(name: string): string {
  const stem = name.replace(/\.[^./]+$/, "");
  return `${stem || "photo"}.jpg`;
}

/** Fit `(w, h)` inside `MAX_EDGE`, never scaling up. */
export function fitWithin(w: number, h: number, maxEdge = MAX_EDGE): { w: number; h: number } {
  const scale = Math.min(1, maxEdge / Math.max(w, h, 1));
  return { w: Math.max(1, Math.round(w * scale)), h: Math.max(1, Math.round(h * scale)) };
}

async function readBytes(blob: Blob): Promise<Uint8Array> {
  return new Uint8Array(await blob.arrayBuffer());
}

/** Decode through an `<img>` rather than `createImageBitmap`: WebKit decodes
 *  HEIC for the former on every iOS the app runs on, and not reliably for the
 *  latter. */
function decodeImage(file: Blob): Promise<HTMLImageElement> {
  return new Promise((resolve, reject) => {
    const url = URL.createObjectURL(file);
    const img = new Image();
    img.onload = () => {
      URL.revokeObjectURL(url);
      resolve(img);
    };
    img.onerror = () => {
      URL.revokeObjectURL(url);
      reject(new Error("image could not be decoded"));
    };
    img.src = url;
  });
}

async function reencode(file: File): Promise<PreparedFile> {
  const img = await decodeImage(file);
  const { w, h } = fitWithin(img.naturalWidth, img.naturalHeight);
  const canvas = document.createElement("canvas");
  canvas.width = w;
  canvas.height = h;
  const ctx = canvas.getContext("2d");
  if (!ctx) throw new Error("no 2d canvas");
  ctx.drawImage(img, 0, 0, w, h);
  const blob = await new Promise<Blob | null>((resolve) =>
    canvas.toBlob(resolve, "image/jpeg", JPEG_QUALITY),
  );
  if (!blob) throw new Error("image could not be encoded");
  return { name: jpegName(file.name), bytes: await readBytes(blob) };
}

/** The bytes to upload for `file`, and the name to stage them under. A
 *  re-encode that fails (an image WebKit cannot decode after all) falls back
 *  to the original bytes rather than losing the attachment. */
export async function prepareFile(file: File): Promise<PreparedFile> {
  if (shouldReencode(file)) {
    try {
      return await reencode(file);
    } catch {
      // Fall through: send what we have.
    }
  }
  return { name: file.name || "attachment", bytes: await readBytes(file) };
}
