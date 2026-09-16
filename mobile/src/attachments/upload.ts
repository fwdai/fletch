// One file, from the phone to the Mac's staging area, in slices small enough
// for the 4 MiB frame cap (docs/remote-protocol.md, "Attachments"). Pure
// against an `AttachmentApi` so the tests can drive it without a host.

import { bytesToBase64 } from "../dictation/encode";

/** Slice size before base64. Inflates to about 1.4 MiB on the wire — under
 *  the frame cap with room for the envelope, and under the host's 2 MiB
 *  decoded-chunk limit. */
export const CHUNK_BYTES = 1024 * 1024;

/** The four ops `uploadAttachment` needs, as the store's api exposes them. */
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
