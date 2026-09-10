import { invoke } from "../invoke";
import type {
  CheckoutFile,
  CheckoutFileContents,
  DiffBaseMode,
  DirListing,
} from "../types/checkout";

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
  /** Write pasted clipboard bytes to the app's attachments dir; resolves to the
   *  absolute path so it can be staged like a dropped file. The name goes in a
   *  header (raw-body IPC), so it's reduced to a plain ASCII basename here. */
  savePastedAttachment: (name: string, bytes: Uint8Array) =>
    invoke<string>("save_pasted_attachment", bytes, {
      headers: { name: name.replace(/[^A-Za-z0-9._-]/g, "_") },
    }),
};
