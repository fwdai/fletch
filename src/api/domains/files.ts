import { invoke } from "../invoke";
import type { CheckoutFile, CheckoutFileContents, DirListing } from "../types/checkout";

export const filesApi = {
  listCheckoutTree: (agentId: string) => invoke<CheckoutFile[]>("list_checkout_tree", { agentId }),
  listDir: (path: string) => invoke<DirListing>("list_dir", { path }),
  // Draft (new-workspace) composer variants, keyed by repo path since a draft
  // has no agent/checkout yet.
  listRepoTree: (repoPath: string) => invoke<string[]>("list_repo_tree", { repoPath }),
  readCheckoutFile: (agentId: string, path: string) =>
    invoke<CheckoutFileContents>("read_checkout_file", { agentId, path }),
  getFileDiff: (agentId: string, path: string) =>
    invoke<string>("get_file_diff", { agentId, path }),
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
};
