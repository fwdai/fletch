import { activeEnvironment } from "@/store/environments";
import {
  type AttachmentApi,
  assertWithinUploadCap,
  uploadAttachment,
} from "@/util/attachmentUpload";
import { invoke } from "../invoke";
import { invokeLocalRaw } from "../transport";
import type {
  CheckoutFile,
  CheckoutFileContents,
  DiffBaseMode,
  DirListing,
} from "../types/checkout";

/** The four upload ops on the environment being driven — remote-only, so these
 *  are never reached for the local one (see `savePastedAttachment`). */
const remoteAttachments: AttachmentApi = {
  attachmentBegin: (name) => invoke<{ upload: string }>("attachment_begin", { name }),
  attachmentChunk: (upload, data) => invoke<null>("attachment_chunk", { upload, data }),
  attachmentEnd: (upload) => invoke<{ path: string }>("attachment_end", { upload }),
  attachmentCancel: (upload) => invoke<null>("attachment_cancel", { upload }),
};

export const filesApi = {
  listCheckoutTree: (agentId: string) => invoke<CheckoutFile[]>("list_checkout_tree", { agentId }),
  listDir: (path: string) => invoke<DirListing>("list_dir", { path }),
  // Draft (new-workspace) composer variants, keyed by repo path since a draft
  // has no agent/checkout yet.
  listRepoTree: (repoPath: string) => invoke<string[]>("list_repo_tree", { repoPath }),
  readCheckoutFile: (agentId: string, path: string, baseMode?: DiffBaseMode) =>
    invoke<CheckoutFileContents>("read_checkout_file", { agentId, path, baseMode }),
  getFileDiff: (agentId: string, path: string, baseMode?: DiffBaseMode) =>
    invoke<string>("get_file_diff", { agentId, path, baseMode }),
  writeCheckoutFile: (agentId: string, path: string, contents: string) =>
    invoke<void>("write_checkout_file", { agentId, path, contents }),
  renameCheckoutPath: (agentId: string, from: string, to: string) =>
    invoke<void>("rename_checkout_path", { agentId, from, to }),
  deleteCheckoutPath: (agentId: string, path: string) =>
    invoke<void>("delete_checkout_path", { agentId, path }),
  createCheckoutFile: (agentId: string, path: string) =>
    invoke<void>("create_checkout_file", { agentId, path }),
  createCheckoutDir: (agentId: string, path: string) =>
    invoke<void>("create_checkout_dir", { agentId, path }),
  copyCheckoutFile: (agentId: string, from: string, to: string) =>
    invoke<void>("copy_checkout_file", { agentId, from, to }),
  /** Stage `bytes` for the active environment and resolve with a path THAT
   *  environment can read — which is the whole point of branching here: the
   *  message that carries the path (`send_user_message`, `wf_launch`) is
   *  routed, so a path on this Mac reaches a host that cannot open it.
   *
   *  On this Mac: raw-body IPC into the app's attachments dir, with the name in
   *  a header (hence the reduction to a plain ASCII basename). On a host: the
   *  `attachment_*` upload of docs/remote-protocol.md, whose `attachment_end`
   *  answers with the staged host path. */
  savePastedAttachment: async (name: string, bytes: Uint8Array): Promise<string> => {
    if (activeEnvironment().kind === "local") {
      return invokeLocalRaw<string>("save_pasted_attachment", bytes, {
        headers: { name: name.replace(/[^A-Za-z0-9._-]/g, "_") },
      });
    }
    // Refused here rather than after a minute of chunking: the host drops an
    // upload over the same cap, and a truncated file is a corrupt one.
    assertWithinUploadCap({ name, size: bytes.length });
    return uploadAttachment(remoteAttachments, name, bytes);
  },
};
