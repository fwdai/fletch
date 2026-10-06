import type { LinkedWorktree } from "@/api";
import { basename } from "@/util/format";

/** "2 sub-agent worktrees · feat/a, feat/b" — each worktree by its branch, or
 *  its directory name when detached; null when there are none. A long list
 *  ellipsizes in CSS, with every path in the line's tooltip. */
export function worktreesLine(worktrees: LinkedWorktree[]): string | null {
  if (worktrees.length === 0) return null;
  const names = worktrees.map((wt) => wt.branch ?? basename(wt.path)).join(", ");
  const noun = worktrees.length === 1 ? "worktree" : "worktrees";
  return `${worktrees.length} sub-agent ${noun} · ${names}`;
}

/** One quiet line under the status header while sub-agents work in their own
 *  worktrees. Their changes never reach this checkout's status, so without it
 *  a busy workspace reads as a clean tree. Informational only — no actions. */
export function WorktreesNote({ worktrees }: { worktrees: LinkedWorktree[] | undefined }) {
  const line = worktreesLine(worktrees ?? []);
  if (!line) return null;
  return (
    <div className="git-wt-note text-xs" title={worktrees?.map((wt) => wt.path).join("\n")}>
      {line}
    </div>
  );
}
