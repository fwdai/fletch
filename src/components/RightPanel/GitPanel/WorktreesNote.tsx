import type { LinkedWorktree } from "@/api";
import { basename } from "@/util/format";

/** How many worktree names the line spells out before folding the rest into "+K". */
const MAX_NAMES = 3;

/** "2 sub-agent worktrees working · feat/a, feat/b" — each worktree by its
 *  branch, or its directory name when detached; null when there are none. */
export function worktreesLine(worktrees: LinkedWorktree[]): string | null {
  if (worktrees.length === 0) return null;
  const names = worktrees.map((wt) => wt.branch ?? basename(wt.path));
  const shown = names.slice(0, MAX_NAMES).join(", ");
  const more = names.length > MAX_NAMES ? ` +${names.length - MAX_NAMES}` : "";
  const noun = worktrees.length === 1 ? "worktree" : "worktrees";
  return `${worktrees.length} sub-agent ${noun} working · ${shown}${more}`;
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
