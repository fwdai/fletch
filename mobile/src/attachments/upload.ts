// The upload flow is the same on both clients, so it lives in the desktop
// tree and is re-exported here — the pattern `lib/paths.ts` follows.

export {
  type AttachmentApi,
  CHUNK_BYTES,
  UploadAborted,
  uploadAttachment,
} from "@desktop/util/attachmentUpload";
