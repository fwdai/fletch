/** One file in the checkout, as returned by `list_checkout_tree`.
 *  `status` is the single-letter git status vs the parent branch
 *  ("M" | "A" | "D" | "R"), or null when the file is unchanged.
 *  Multi-repo agents get paths prefixed with the owning checkout's subdir
 *  ("<subdir>/<rel>"); the file read/write commands resolve the same prefix
 *  back, so the panel can pass these paths through unchanged. */
export interface CheckoutFile {
  path: string;
  status: string | null;
  additions: number;
  deletions: number;
}

/** One entry in an arbitrary directory listing, used by the composer's `@`
 *  file-mention autocomplete when the user types a filesystem path. */
export interface DirEntry {
  name: string;
  is_dir: boolean;
}

/** A directory listing plus the absolute (tilde-expanded) path that was
 *  read, returned by `list_dir`. */
export interface DirListing {
  base: string;
  entries: DirEntry[];
}

/** A checkout file's contents plus the metadata the File-panel editor
 *  needs. `chg_add` / `chg_mod` are 1-indexed line numbers the agent
 *  added / modified (drives the change gutter). */
export interface CheckoutFileContents {
  text: string;
  lang: string;
  status: string | null;
  chg_add: number[];
  chg_mod: number[];
  binary: boolean;
  too_large: boolean;
}
