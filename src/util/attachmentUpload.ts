// Staging a file on a host: the `attachment_begin|chunk|end|cancel` flow of
// docs/remote-protocol.md's "Attachments", as a pure driver over four calls.
//
// Shared by both clients — the phone, which has no path to send in the first
// place, and the desktop while a host is the active environment, where a path
// on this Mac names nothing the host can read. Neither of them owns the flow,
// so it lives here beside the other helpers mobile imports through `@desktop`
// (util/paths, util/folderBrowser).

import { bytesToBase64 } from "./base64";

/** Slice size before base64. Inflates to about 1.4 MiB on the wire — under
 *  the frame cap with room for the envelope, and under the host's 2 MiB
 *  decoded-chunk limit. */
export const CHUNK_BYTES = 1024 * 1024;

/** The host's per-file cap (`attachments::remote::MAX_UPLOAD_BYTES`), applied
 *  by the client before a byte is read: a video or archive picked by mistake
 *  would otherwise be pulled whole into the webview's memory and then uploaded
 *  up to the cap before the host refused it. */
export const MAX_UPLOAD_BYTES = 32 * 1024 * 1024;

const mb = (bytes: number) => `${Math.round(bytes / (1024 * 1024))} MB`;

/** Refuse a file the host would refuse, in the words the composer shows. */
export function assertWithinUploadCap(file: { name: string; size: number }): void {
  if (file.size > MAX_UPLOAD_BYTES) {
    throw new Error(
      `${file.name || "file"} is ${mb(file.size)} — the limit is ${mb(MAX_UPLOAD_BYTES)}`,
    );
  }
}

/** The four ops `uploadAttachment` needs, as each app's api exposes them. */
export interface AttachmentApi {
  attachmentBegin(name: string): Promise<{ upload: string }>;
  attachmentChunk(upload: string, data: string): Promise<null>;
  attachmentEnd(upload: string): Promise<{ path: string }>;
  attachmentCancel(upload: string): Promise<null>;
}

/** The error an aborted upload rejects with; the caller that aborted it has
 *  nothing to show for it. */
export class UploadAborted extends Error {
  constructor() {
    super("attachment upload cancelled");
    this.name = "UploadAborted";
  }
}

/** Stage `bytes` on the host under `name` and resolve with the staged path,
 *  which goes in `send_user_message`'s `attachments`. Chunks go out one at a
 *  time, in order — the host appends as they arrive. Any failure, or an abort
 *  through `signal`, tells the host to drop the partial upload before
 *  rejecting, so nothing half-written is left behind. */
export async function uploadAttachment(
  api: AttachmentApi,
  name: string,
  bytes: Uint8Array,
  signal?: AbortSignal,
): Promise<string> {
  if (signal?.aborted) throw new UploadAborted();
  const { upload } = await api.attachmentBegin(name);
  try {
    for (let at = 0; at < bytes.length; at += CHUNK_BYTES) {
      if (signal?.aborted) throw new UploadAborted();
      await api.attachmentChunk(upload, bytesToBase64(bytes.subarray(at, at + CHUNK_BYTES)));
    }
    if (signal?.aborted) throw new UploadAborted();
    return (await api.attachmentEnd(upload)).path;
  } catch (e) {
    await api.attachmentCancel(upload).catch(() => {});
    throw e;
  }
}
