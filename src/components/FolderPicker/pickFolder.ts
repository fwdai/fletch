// Asking the user for a folder, in whichever environment the UI is driving.
// This Mac's native picker cannot see a paired host's disk, so on a remote
// environment the question is asked by the in-app browser instead (index.tsx),
// which walks the host over `list_dir`.
//
// The modal is a singleton mounted once at the app root, so the request lives
// here — module state with a subscription, read by `FolderPickerHost` through
// `useSyncExternalStore`. Nothing else in the app needs to know which of the
// two pickers answered.

import { open } from "@tauri-apps/plugin-dialog";
import { activeEnvironment } from "@/store/environments";

export interface FolderRequest {
  title: string;
  /** Where the browser opens. `~` is resolved by the host. */
  start: string;
  resolve: (path: string | null) => void;
}

let pending: FolderRequest | null = null;
const listeners = new Set<() => void>();

export function subscribeFolderRequest(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

export function folderRequest(): FolderRequest | null {
  return pending;
}

/** Answer the open request — `null` when the user backed out. */
export function resolveFolderRequest(path: string | null): void {
  const request = pending;
  pending = null;
  for (const listener of listeners) listener();
  request?.resolve(path);
}

/** Pick a folder on the active environment: the native directory dialog on this
 *  Mac, the in-app browser on a paired host. Resolves to an absolute path — the
 *  one the host itself reported, remotely — or null if nothing was chosen. */
export async function pickFolder({
  title,
  start,
}: {
  title: string;
  start?: string;
}): Promise<string | null> {
  if (activeEnvironment().kind === "local") {
    const picked = await open({
      directory: true,
      multiple: false,
      title,
      defaultPath: start || undefined,
    });
    return typeof picked === "string" ? picked : null;
  }
  // One at a time: a second request while the modal is up would strand the
  // first on a promise nothing can settle.
  resolveFolderRequest(null);
  return new Promise((resolve) => {
    pending = { title, start: start || "~", resolve };
    for (const listener of listeners) listener();
  });
}
