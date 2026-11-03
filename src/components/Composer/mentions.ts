// Pure helpers for the composer's "@" file-mention autocomplete: deciding
// whether a query is a filesystem path and ranking the candidates. Trigger
// detection itself lives in `autocomplete/triggers.ts` (shared by all
// sources); picking attaches the file or drills into a directory.

/** A mention query is a filesystem path (resolved via `list_dir`) rather
 *  than a checkout-file search when it starts with `~`, `/`, `./` or `../`. */
export function isFsPath(query: string): boolean {
  return /^(~|\/|\.\/|\.\.\/)/.test(query);
}

/** The `@~/…` and `@/…` paths written into `text`, tokenized like the `@`
 *  trigger (`triggerQueryAt`): mentions typed out, or left as text when the
 *  menu closed without a pick. Relative ones (`./`, `../`) are left out, since
 *  in a message they mean the checkout, not a folder on this Mac. Sentence
 *  punctuation after a path is not part of it. */
export function pathMentions(text: string): string[] {
  const found = new Set<string>();
  for (const m of text.matchAll(/(?:^|\s)@([^\s@#]+)/g)) {
    const path = m[1].replace(/[.,;:!?)\]}'"]+$/, "");
    if (path === "~" || path.startsWith("~/") || path.startsWith("/")) found.add(path);
  }
  return [...found];
}

/** Whether the directory a listing resolved to `base`, typed as `typed`, may
 *  be given to the agent from the composer: never `/`, a top-level folder like
 *  `/Users`, or home as typed (`~`). The rest of what a grant may not name
 *  (home by another path, hidden folders, …) is the backend's to refuse, with
 *  why. */
export function offersFolder(base: string, typed: string): boolean {
  const dir = base.replace(/\/+$/, "");
  const home = typed.replace(/\/+$/, "") === "~";
  return !home && dir.split("/").length >= 3;
}

/** Split a typed filesystem path into the directory to list and the partial
 *  basename to filter by. The `dir` keeps the user's prefix (e.g. `~`) so we
 *  can re-display it; `list_dir` resolves the real path. */
export function splitFsPath(query: string): { dir: string; partial: string } {
  const slash = query.lastIndexOf("/");
  if (slash === -1) return { dir: query, partial: "" };
  return { dir: query.slice(0, slash) || "/", partial: query.slice(slash + 1) };
}

/** Append `name` to a typed directory, normalizing the separator (so `~` →
 *  `~/name`, `/` → `/name`, `~/Downloads` → `~/Downloads/name`). */
export function joinTypedDir(dir: string, name: string): string {
  return (dir.endsWith("/") ? dir : `${dir}/`) + name;
}

/** Rank directory entries against a partial basename for the `@` menu:
 *  directories first, then prefix matches, then substrings, alpha within.
 *  Dotfiles are hidden unless the partial itself starts with ".". */
export function filterDirEntries<T extends { name: string; is_dir: boolean }>(
  entries: T[],
  partial: string,
  limit = 10,
): T[] {
  const q = partial.toLowerCase();
  const showHidden = partial.startsWith(".");
  const scored: { e: T; score: number }[] = [];
  for (const e of entries) {
    if (!showHidden && e.name.startsWith(".")) continue;
    const lower = e.name.toLowerCase();
    let score: number;
    if (!q) score = 0;
    else if (lower.startsWith(q)) score = 0;
    else if (lower.includes(q)) score = 1;
    else continue;
    scored.push({ e, score });
  }
  scored.sort(
    (a, b) =>
      Number(b.e.is_dir) - Number(a.e.is_dir) ||
      a.score - b.score ||
      (a.e.name < b.e.name ? -1 : 1),
  );
  return scored.slice(0, limit).map((s) => s.e);
}

/** True when `query`'s characters appear in `text` in order (fuzzy match). */
function isSubsequence(text: string, query: string): boolean {
  let i = 0;
  for (let j = 0; j < text.length && i < query.length; j++) {
    if (text[j] === query[i]) i++;
  }
  return i === query.length;
}

/** Rank checkout file paths against a mention query. Best matches first:
 *  basename prefix, then basename substring, then anywhere in the path,
 *  then a fuzzy subsequence fallback. Ties break on the shorter, then
 *  alphabetically-earlier, path. An empty query lists the first `limit`. */
export function filterFiles(files: string[], query: string, limit = 8): string[] {
  const q = query.toLowerCase();
  if (!q) return files.slice(0, limit);

  const scored: { path: string; score: number }[] = [];
  for (const path of files) {
    const lower = path.toLowerCase();
    const base = lower.slice(lower.lastIndexOf("/") + 1);
    const baseIdx = base.indexOf(q);
    let score: number;
    if (baseIdx === 0) score = 0;
    else if (baseIdx > 0) score = 1;
    else if (lower.includes(q)) score = 2;
    else if (isSubsequence(lower, q)) score = 3;
    else continue;
    scored.push({ path, score });
  }

  scored.sort(
    (a, b) => a.score - b.score || a.path.length - b.path.length || (a.path < b.path ? -1 : 1),
  );
  return scored.slice(0, limit).map((s) => s.path);
}
