import { activeEnvironment } from "@/store/environments";
import {
  type AttachmentApi,
  assertWithinUploadCap,
  uploadAttachment,
} from "@/util/attachmentUpload";
import { invoke } from "../invoke";
import { activeTransport, invokeLocalRaw, type Transport } from "../transport";
import type {
  CheckoutFile,
  CheckoutFileContents,
  DiffBaseMode,
  DirListing,
} from "../types/checkout";

/** The four upload ops bound to ONE environment — remote-only, so this is never
 *  reached for the local one (see `savePastedAttachment`). Bound rather than
 *  resolved per call because an upload is a stateful sequence: the id
 *  `attachment_begin` answers with exists only on the host that issued it, so a
 *  later chunk/end/cancel sent to another host names nothing there. */
const attachmentApiOver = (transport: Transport): AttachmentApi => ({
  attachmentBegin: (name) => transport.call<{ upload: string }>("attachment_begin", { name }),
  attachmentChunk: (upload, data) => transport.call<null>("attachment_chunk", { upload, data }),
  attachmentEnd: (upload) => transport.call<{ path: string }>("attachment_end", { upload }),
  attachmentCancel: (upload) => transport.call<null>("attachment_cancel", { upload }),
});

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
    // Captured once, here: a switch while the chunks are going up must not send
    // the rest of this upload to the other host. Nothing else needs invalidating
    // when that happens — the composer's attachment list is per-environment
    // through the stash (store/environmentSwitch), so the path this resolves
    // with lands in the composer of the environment it was staged on, and the
    // one on screen never shows it.
    return uploadAttachment(attachmentApiOver(activeTransport()), name, bytes);
  },
};
