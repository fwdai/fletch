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
  /** What the restore does, in a sentence. */
  summary: string;
  changes: BranchChange[];
  /** A leaving commit was already pushed, so its branch's next push has to
   *  force. */
  pushed: boolean;
  /** One row per checkout that kept no snapshot of the message, which the
   *  restore leaves as it is: named, so a partial restore is confirmed
   *  knowingly rather than leaving a silently mixed workspace. */
  keptAsIs: string[];
}

/** The confirmation for rewinding with `scope`, from the restore's preview. */
export function restoreConfirmation(scope: CodeScope, report: RestoreReport): RestoreConfirmation {
  const changes = report.repos
    .filter((repo) => repo.checkpoint !== null && repo.leaving.length > 0)
    .map(({ subdir, branch, leaving }) => ({ subdir, branch, leaving }));
  const keptAsIs = report.repos
    .filter((repo) => repo.checkpoint === null)
    .map((repo) => `${repo.subdir}: no snapshot of this message, so it stays as it is now`);
  const both = scope === "both";
  return {
    title: both ? "Restore the conversation and code?" : "Restore the code?",
    action: both ? "Restore conversation and code" : "Restore code",
    summary:
      keptAsIs.length === 0
        ? "Each checkout goes back to how it was when this message was sent."
        : "Only part of the code goes back: the checkouts with a snapshot of this message return to how they were when it was sent, and the others stay as they are now.",
    changes,
    pushed: changes.some((change) => change.leaving.some((commit) => commit.pushed)),
    keptAsIs,
  };
}
