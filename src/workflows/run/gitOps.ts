// run/gitOps.ts — typed wrappers over the workflow git backend commands.

import { invoke } from "@tauri-apps/api/core";

/** Locate a step's worktree server-side from the agent id + its repo subdir. */
export interface StepRef {
  agentId: string;
  subdir: string;
}

/** Ensure `.quorum/` is git-excluded locally for this repo (idempotent). */
export const prepareRepo = (repoPath: string) =>
  invoke<{ ok: boolean }>("workflow_prepare_repo", { repoPath });

/** Copy the previous step's `.quorum/` notes into the next step's worktree. */
export const ferryNotes = (from: StepRef, to: StepRef) =>
  invoke<{ ok: boolean }>("workflow_ferry_notes", {
    fromAgentId: from.agentId,
    fromSubdir: from.subdir,
    toAgentId: to.agentId,
    toSubdir: to.subdir,
  });

/** Stage + commit if dirty; returns the resulting HEAD and whether it committed. */
export const boundaryCommit = (ref: StepRef, message: string) =>
  invoke<{ head: string; committed: boolean }>("workflow_boundary_commit", {
    agentId: ref.agentId,
    subdir: ref.subdir,
    message,
  });

export const headSha = (ref: StepRef) =>
  invoke<{ head: string }>("workflow_head_sha", {
    agentId: ref.agentId,
    subdir: ref.subdir,
  });

/** True if `path` (relative to the worktree) exists — backs file/loop gates. */
export const fileExists = (ref: StepRef, path: string) =>
  invoke<{ exists: boolean }>("workflow_file_exists", {
    agentId: ref.agentId,
    subdir: ref.subdir,
    path,
  });

export const finalize = (
  ref: StepRef,
  opts: { branch: string; baseBranch?: string; title?: string; body?: string },
) =>
  invoke<{ pushed: boolean; branch: string; pr: string | null; pr_error: string | null }>(
    "workflow_finalize",
    {
      agentId: ref.agentId,
      subdir: ref.subdir,
      branch: opts.branch,
      baseBranch: opts.baseBranch,
      title: opts.title,
      body: opts.body,
    },
  );
