import { describe, expect, it } from "vitest";
import { worktreesLine } from "./WorktreesNote";

const wt = (name: string, branch: string | null = name) => ({
  path: `/c/.claude/worktrees/${name}`,
  branch,
});

describe("worktreesLine", () => {
  it("says nothing without worktrees", () => {
    expect(worktreesLine([])).toBeNull();
  });

  it("names a detached worktree by its directory", () => {
    expect(worktreesLine([wt("agent-a", null)])).toBe("1 sub-agent worktree · agent-a");
  });

  it("names every worktree", () => {
    const line = worktreesLine([wt("feat/a"), wt("feat/b"), wt("feat/c"), wt("x")]);
    expect(line).toBe("4 sub-agent worktrees · feat/a, feat/b, feat/c, x");
  });
});
