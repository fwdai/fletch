// Pure helpers for the composer's "@" file-mention autocomplete.
//
// Typing "@" (at the start, or after whitespace) opens a menu of the
// agent's worktree files. Picking one removes the "@query" text and adds
// the file to the message attachments — matching the existing attach flow,
// so the agent receives it as a separate "Attached file: <path>" block.

/** The active "@" mention under the caret, or null when there isn't one.
 *  `start` is the index of the "@" itself, so callers can splice out the
 *  whole `@query` span. The "@" must be at the start of the text or
 *  preceded by whitespace, so addresses like `foo@bar` don't trigger it. */
export function mentionQueryAt(
  text: string,
  caret: number,
): { query: string; start: number } | null {
  const upto = text.slice(0, caret);
  const m = /(?:^|\s)@([^\s@]*)$/.exec(upto);
  if (!m) return null;
  const query = m[1];
  return { query, start: caret - query.length - 1 };
}

/** A mention query is a filesystem path (resolved via `list_dir`) rather
 *  than a worktree-file search when it starts with `~`, `/`, `./` or `../`. */
export function isFsPath(query: string): boolean {
  return /^(~|\/|\.\/|\.\.\/)/.test(query);
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
  return (dir.endsWith("/") ? dir : dir + "/") + name;
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

/** Index where the "@" token under the caret ends: the first whitespace or
 *  "@" at or after `from`, else the end of the text. Mirrors the `[^\s@]`
 *  token class `mentionQueryAt` uses, so picking removes the *whole* token
 *  even when the caret was moved back into the middle of it (otherwise the
 *  tail past the caret, e.g. "ponents" in "@components", would survive). */
export function mentionTokenEnd(text: string, from: number): number {
  let end = from;
  while (end < text.length && !/[\s@]/.test(text[end])) end++;
  return end;
}

/** True when `query`'s characters appear in `text` in order (fuzzy match). */
function isSubsequence(text: string, query: string): boolean {
  let i = 0;
  for (let j = 0; j < text.length && i < query.length; j++) {
    if (text[j] === query[i]) i++;
  }
  return i === query.length;
}

/** Rank worktree file paths against a mention query. Best matches first:
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
    (a, b) =>
      a.score - b.score ||
      a.path.length - b.path.length ||
      (a.path < b.path ? -1 : 1),
  );
  return scored.slice(0, limit).map((s) => s.path);
}
