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
    expect(worktreesLine([wt("agent-a", null)])).toBe("1 sub-agent worktree working · agent-a");
  });

  it("spells out three names and folds the rest", () => {
    const line = worktreesLine([wt("feat/a"), wt("feat/b"), wt("feat/c"), wt("x"), wt("y")]);
    expect(line).toBe("5 sub-agent worktrees working · feat/a, feat/b, feat/c +2");
  });
});
