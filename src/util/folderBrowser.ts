// Walking a Fletch engine's disk over `list_dir`, shared by the desktop's
// remote folder picker and the phone's Add Project sheet. The `listDir` call is
// a parameter, so this knows nothing about which store or transport answers it.

import { useEffect, useState } from "react";
import type { DirEntry, DirListing } from "@/api/types/checkout";
import { parentPath } from "./paths";

const isHidden = (entry: DirEntry) => entry.name.startsWith(".");

/** What a picker shows of a listing: the directories only — everything these
 *  pickers do takes a folder — sorted by name, with the dot-folders held back
 *  behind a toggle and counted so the toggle can say how many. Pure, so the
 *  filtering is testable without a renderer. */
export function folderView(listing: DirListing | null, showHidden: boolean) {
  const dirs = (listing?.entries ?? [])
    .filter((e) => e.is_dir)
    .sort((a, b) => a.name.localeCompare(b.name));
  const hiddenCount = dirs.filter(isHidden).length;
  return { hiddenCount, shown: showHidden ? dirs : dirs.filter((e) => !isHidden(e)) };
}

export interface FolderBrowserOptions {
  /** Reads one directory on the engine being browsed. Must be stable across
   *  renders — it is a dependency of the read effect. */
  listDir: (path: string) => Promise<DirListing>;
  /** Where to start. `~` is resolved by the host, not here. */
  start?: string;
  /** Keep the last listing on a failed read instead of clearing it. For a
   *  picker whose path can be typed (the desktop's), where a typo should not
   *  empty the list the user was browsing; the phone's taps can only reach
   *  folders the host just named, so there it drops the stale listing and the
   *  crumb follows the folder that failed. */
  keepListingOnError?: boolean;
}

/** Browse a directory tree: where we are, what is in it, and what failed. The
 *  walk is anchored on the `base` the host reports, never on the string it was
 *  asked for — `~` and symlinks resolve there, not here. */
export function useFolderBrowser({
  listDir,
  start = "~",
  keepListingOnError = false,
}: FolderBrowserOptions) {
  const [path, setPath] = useState(start);
  const [listing, setListing] = useState<DirListing | null>(null);
  const [loading, setLoading] = useState(true);
  const [showHidden, setShowHidden] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    setLoading(true);
    setError(null);
    listDir(path).then(
      (next) => {
        if (!live) return;
        setListing(next);
        setLoading(false);
      },
      (e: unknown) => {
        if (!live) return;
        if (!keepListingOnError) setListing(null);
        setError(e instanceof Error ? e.message : String(e));
        setLoading(false);
      },
    );
    return () => {
      live = false;
    };
  }, [path, listDir, keepListingOnError]);

  const base = listing?.base ?? path;
  return {
    /** The path asked for — which is what the user typed or tapped, and not
     *  necessarily what came back. */
    path,
    setPath,
    listing,
    loading,
    error,
    showHidden,
    setShowHidden,
    /** The absolute, host-resolved directory the listing is of. */
    base,
    up: parentPath(base),
    ...folderView(listing, showHidden),
  };
}
