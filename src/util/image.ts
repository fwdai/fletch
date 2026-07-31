// Pick an image from disk and hand back a size-bounded JPEG, base64-encoded.
//
// Written for the feedback modal's screenshot attachment, but deliberately
// generic: any surface that needs "let the user attach an image, small enough to
// ship over IPC" can call `pickImageAsBase64`.
//
// The re-encode is not optional. A retina `⌘⇧4` PNG is routinely 3-5 MB, while
// the feedback event's transport (PostHog) drops anything over 1 MB — so we
// downscale and JPEG-encode in the webview, which is already a complete image
// pipeline, rather than pulling an image crate into the Rust side.

import { open } from "@tauri-apps/plugin-dialog";
import { readFile } from "@tauri-apps/plugin-fs";

const IMAGE_FILTERS = [{ name: "Image", extensions: ["png", "jpg", "jpeg", "webp", "gif"] }];

/** Longest edge of the re-encoded image. 1600px keeps UI text legible in a
 *  screenshot while cutting a 2x 3024px capture to a third of its pixels. */
const DEFAULT_MAX_DIM = 1600;

/** Quality ladder, tried in order until the encode fits `maxBytes`. Screenshots
 *  are flat UI, so 0.72 almost always lands well under the cap on the first try;
 *  the lower rungs exist for dense photographic content. */
const QUALITY_LADDER = [0.72, 0.6, 0.45];

export interface PickedImage {
  /** Base64 (no data-URL prefix) of the re-encoded JPEG — what goes over IPC. */
  base64: string;
  /** `data:image/jpeg;base64,…`, for an `<img src>` preview. */
  dataUrl: string;
  /** Encoded size in bytes of the base64 payload (what the cap applies to). */
  bytes: number;
  /** Basename of the file the user picked, for display. */
  name: string;
}

export interface PickImageOptions {
  /** Cap on the base64 payload, in bytes. */
  maxBytes: number;
  /** Longest-edge cap in pixels. Defaults to {@link DEFAULT_MAX_DIM}. */
  maxDim?: number;
  title?: string;
}

/** Scale `w`×`h` down to fit a `max`×`max` box, preserving aspect ratio. Never
 *  scales up, and never returns a zero dimension (a canvas of width 0 throws). */
export function fitWithin(w: number, h: number, max: number): { w: number; h: number } {
  const longest = Math.max(w, h);
  if (longest <= max || longest === 0) return { w, h };
  const scale = max / longest;
  return { w: Math.max(1, Math.round(w * scale)), h: Math.max(1, Math.round(h * scale)) };
}

/** Basename of a filesystem path, for display next to the thumbnail. */
function basename(path: string): string {
  return path.split(/[\\/]/).pop() || path;
}

/** Encode a canvas to a JPEG blob at `quality`. Wraps the callback-style
 *  `toBlob`, which yields `null` if the browser can't encode. */
function toJpegBlob(canvas: HTMLCanvasElement, quality: number): Promise<Blob> {
  return new Promise((resolve, reject) => {
    canvas.toBlob(
      (blob) => (blob ? resolve(blob) : reject(new Error("Could not encode the image."))),
      "image/jpeg",
      quality,
    );
  });
}

async function blobToBase64(blob: Blob): Promise<string> {
  const buf = new Uint8Array(await blob.arrayBuffer());
  // Chunked so a multi-hundred-KB image can't blow the argument limit of
  // `String.fromCharCode(...)`.
  let binary = "";
  const CHUNK = 0x8000;
  for (let i = 0; i < buf.length; i += CHUNK) {
    binary += String.fromCharCode(...buf.subarray(i, i + CHUNK));
  }
  return btoa(binary);
}

/**
 * Prompt for an image file, then downscale + JPEG-encode it under `maxBytes`.
 * Resolves `null` if the user dismissed the picker. Throws with a
 * user-presentable message if the file isn't a decodable image, or if even the
 * lowest quality won't fit.
 */
export async function pickImageAsBase64(opts: PickImageOptions): Promise<PickedImage | null> {
  const path = await open({
    title: opts.title ?? "Attach an image",
    multiple: false,
    directory: false,
    filters: IMAGE_FILTERS,
  });
  if (typeof path !== "string") return null;

  // The dialog plugin grants this exact path to the fs scope when the user picks
  // it (`allow_file`), which is why a `readFile` of an arbitrary path works here
  // without widening the capability — same mechanism the workflow YAML import
  // relies on.
  const bytes = await readFile(path);
  // `readFile` returns a view into a larger buffer; slice it so the Blob can't
  // pick up neighbouring bytes.
  const source = new Blob([bytes.slice().buffer]);

  let bitmap: ImageBitmap;
  try {
    bitmap = await createImageBitmap(source);
  } catch {
    throw new Error(`Couldn't read ${basename(path)} as an image.`);
  }

  try {
    const { w, h } = fitWithin(bitmap.width, bitmap.height, opts.maxDim ?? DEFAULT_MAX_DIM);
    const canvas = document.createElement("canvas");
    canvas.width = w;
    canvas.height = h;
    const ctx = canvas.getContext("2d");
    if (!ctx) throw new Error("Could not encode the image.");
    // JPEG has no alpha: paint white underneath so a transparent PNG doesn't
    // come out on a black background.
    ctx.fillStyle = "#ffffff";
    ctx.fillRect(0, 0, w, h);
    ctx.drawImage(bitmap, 0, 0, w, h);

    for (const quality of QUALITY_LADDER) {
      const base64 = await blobToBase64(await toJpegBlob(canvas, quality));
      if (base64.length <= opts.maxBytes) {
        return {
          base64,
          dataUrl: `data:image/jpeg;base64,${base64}`,
          bytes: base64.length,
          name: basename(path),
        };
      }
    }
    throw new Error("That image is too large to attach. Try a smaller crop.");
  } finally {
    bitmap.close();
  }
}
