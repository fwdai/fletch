import type { LeavingCommit, RestoreReport, RewindScope } from "@/api";

/** A rewind that restores the code, and so is confirmed first. */
export type CodeScope = Exclude<RewindScope, "conversation">;

/** A checkout whose branch a code restore takes commits off. */
export interface BranchChange {
  subdir: string;
  /** The branch they leave; null for a detached HEAD. */
  branch: string | null;
  /** Newest first. */
  leaving: LeavingCommit[];
}

/** What the confirmation before a code restore says. */
export interface RestoreConfirmation {
  title: string;
  /** The confirm button. */
  action: string;
  changes: BranchChange[];
  /** A leaving commit was already pushed, so its branch's next push has to
   *  force. */
  pushed: boolean;
  /** Checkouts with no snapshot of the message, which stay as they are. */
  untouched: string[];
}

/** The confirmation for rewinding with `scope`, from the restore's preview. */
export function restoreConfirmation(scope: CodeScope, report: RestoreReport): RestoreConfirmation {
  const changes = report.repos
    .filter((repo) => repo.checkpoint !== null && repo.leaving.length > 0)
    .map(({ subdir, branch, leaving }) => ({ subdir, branch, leaving }));
  const both = scope === "both";
  return {
    title: both ? "Restore the conversation and code?" : "Restore the code?",
    action: both ? "Restore conversation and code" : "Restore code",
    changes,
    pushed: changes.some((change) => change.leaving.some((commit) => commit.pushed)),
    untouched: report.repos.filter((repo) => repo.checkpoint === null).map((repo) => repo.subdir),
  };
}
