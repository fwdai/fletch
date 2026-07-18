export interface DiffStats {
  additions: number;
  deletions: number;
}

export type StatusKind = "modified" | "added" | "deleted" | "renamed" | "untracked" | "conflicted";

export interface FileStatus {
  path: string;
  kind: StatusKind;
  staged: boolean;
  additions: number;
  deletions: number;
}

/** Compact projection of GitState used by the app-wide bulk poll —
 *  enough for sidebar shortstats and tab badges without shipping every
 *  agent's file list over IPC. */
export interface ShortStats {
  additions: number;
  deletions: number;
  file_count: number;
}

/** Advisory fleet-wide git metadata for one checkout, keyed by `gitKey` in the
 *  bulk `getAllGitMeta` reply. `behind` (base moved ahead of this checkout) and
 *  `files` (working-tree paths) drive the always-visible staleness chip and the
 *  cross-agent overlap hints. `behind` is null when the base tip can't be
 *  resolved (no GitHub / no fetch yet) — render nothing, never a zero. */
export interface GitMeta {
  base: string;
  behind: number | null;
  files: string[];
}

export interface GitState {
  branch: string;
  parent_branch: string;
  ahead: number;
  behind: number;
  /** Commits on HEAD not yet on the upstream — how many a push would send.
   *  Distinct from `ahead` (measured vs the base branch). */
  unpushed: number;
  files: FileStatus[];
  additions: number;
  deletions: number;
  /** GitHub web base for `origin` (`https://github.com/owner/repo`), or null
   *  when origin is missing / not a github.com remote. Lets the panel link to
   *  a commit or compare view. */
  remote_url?: string | null;
  /** Whether an `origin` remote exists at all (GitHub or not). False = a
   *  local-only repo: push/PR give way to "Publish to GitHub". */
  has_origin: boolean;
  /** HEAD commit SHA, for a single-commit link when one commit is ahead. */
  head_sha?: string | null;
}
