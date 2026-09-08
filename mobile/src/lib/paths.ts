// Absolute-path arithmetic for the folder picker and the mock host's fake
// filesystem. The host answers `list_dir` with the absolute, tilde-expanded
// `base` it actually read, so every path built here is anchored on that rather
// than on whatever the user typed.

/** Join a listing's `base` with an entry name. `base` ends in a separator at
 *  the root and nowhere else, and a doubled one is a different path to some
 *  hosts — so trim before joining. */
export function childPath(base: string, name: string): string {
  return `${base.replace(/\/+$/, "")}/${name}`;
}

/** The containing directory, or null when `path` is already the root. */
export function parentPath(path: string): string | null {
  const trimmed = path.replace(/\/+$/, "");
  const cut = trimmed.lastIndexOf("/");
  if (cut < 0) return null;
  return cut === 0 ? "/" : trimmed.slice(0, cut);
}

/** The last segment of a path — the folder's own name. */
export function baseName(path: string): string {
  const trimmed = path.replace(/\/+$/, "");
  return trimmed.slice(trimmed.lastIndexOf("/") + 1) || "/";
}
