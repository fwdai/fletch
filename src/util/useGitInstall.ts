// Portable-git install state for readiness surfaces: the live bootstrap
// phase (via useGitDist), the derived downloading/failed views, and the
// manual install/retry action. Shared so every surface that shows a git row
// resolves and retries identically.

import { useCallback, useState } from "react";
import { api, type ToolStatus } from "@/api";
import { type GitDistState, useGitDist } from "./useGitDist";

export interface GitInstall {
  /** Live portable-git bootstrap state (download progress / failure). */
  gitDist: GitDistState;
  /** Portable git download in flight (startup bootstrap or manual retry). */
  gitDownloading: boolean;
  /** Why the last install attempt failed, until a retry succeeds. */
  gitInstallError?: string;
  installingGit: boolean;
  installGit: () => void;
}

/** `git` is the caller's latest probe result; `recheck` re-runs its checks —
 *  called when the bootstrap settles and after a manual install attempt. */
export function useGitInstall(git: ToolStatus | null, recheck: () => void): GitInstall {
  // While the app is downloading its portable git (no usable system git),
  // callers show that instead of a false "not found"; re-check on settle.
  const gitDist = useGitDist(recheck);
  const gitDownloading = !git?.installed && gitDist.phase === "downloading";
  const gitInstallError = !git?.installed && gitDist.phase === "failed" ? gitDist.error : undefined;

  const [installingGit, setInstallingGit] = useState(false);
  const installGit = useCallback(() => {
    setInstallingGit(true);
    // Progress + failure reason arrive via git-dist:state; the final recheck
    // covers the case where the bootstrap already settled before mount (no
    // further events) yet the retry succeeded.
    void api
      .gitDistInstall()
      .catch(() => {})
      .finally(() => {
        setInstallingGit(false);
        recheck();
      });
  }, [recheck]);

  return { gitDist, gitDownloading, gitInstallError, installingGit, installGit };
}
