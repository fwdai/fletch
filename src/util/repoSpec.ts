// Parse/validate the clone spec and new-project name in the browser, so the
// New Project flow gives instant feedback. Mirrors the Rust logic in
// `src-tauri/src/new_project.rs` (repo_name_from_spec / validate_new_name).

const NAME_RE = /^[A-Za-z0-9._-]+$/;

/** A GitHub repo name is legal: letters, digits, '.', '-', '_'; not '.'/'..'. */
export function isValidRepoName(name: string): boolean {
  const n = name.trim();
  if (!n || n === "." || n === "..") return false;
  return NAME_RE.test(n);
}

export interface ParsedSpec {
  valid: boolean;
  /** The derived repo (and local dir) name, when valid. */
  name?: string;
}

/**
 * Derive the repo name from a clone spec. Accepts `owner/repo`,
 * `https://github.com/owner/repo(.git)`, and `git@github.com:owner/repo.git`.
 */
export function parseRepoSpec(input: string): ParsedSpec {
  const spec = input.trim();
  if (!spec) return { valid: false };

  const tail = spec.replace(/\/+$/, "").split(/[/:]/).pop() ?? spec;
  const name = tail.replace(/\.git$/, "").trim();
  if (!name || !isValidRepoName(name)) return { valid: false };
  return { valid: true, name };
}
