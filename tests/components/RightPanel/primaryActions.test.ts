import { describe, expect, it } from "vitest";
import type { GitState, PrState } from "@/api";
import { deriveState } from "@/components/RightPanel/primaryActions";

function git(over: Partial<GitState> = {}): GitState {
  return {
    branch: "feat",
    parent_branch: "main",
    ahead: 1,
    behind: 0,
    unpushed: 0,
    files: [],
    additions: 0,
    deletions: 0,
    has_origin: true,
    ...over,
  };
}
const file = (kind: GitState["files"][number]["kind"]) => ({
  path: "a.ts",
  kind,
  staged: false,
  additions: 1,
  deletions: 0,
});
function pr(over: Partial<PrState> = {}): PrState {
  return {
    number: 7,
    url: "https://x",
    state: "open",
    title: "t",
    mergeable: "mergeable",
    ...over,
  };
}

describe("deriveState", () => {
  it("uncommitted changes take precedence over an open PR", () => {
    expect(deriveState(git({ files: [file("modified")] }), pr())).toBe("changes");
    expect(deriveState(git(), pr())).toBe("pr-open");
  });

  it("conflicts still take precedence over changes", () => {
    expect(deriveState(git({ files: [file("conflicted")] }), pr())).toBe("conflicts");
    expect(deriveState(git({ files: [file("conflicted")] }), pr({ state: "merged" }))).toBe(
      "conflicts",
    );
  });

  it("merged claims the state only while nothing has happened since the merge", () => {
    const merged = pr({ state: "merged" });
    expect(deriveState(git(), merged)).toBe("merged");
    // A squash-merge leaves `ahead > 0` against a stale local base with nothing
    // new in the tree — that must NOT read as follow-up work.
    expect(deriveState(git({ ahead: 3 }), merged)).toBe("merged");
  });

  it("keeps working after a merge: follow-up work moves the state off merged", () => {
    const merged = pr({ state: "merged" });
    expect(deriveState(git({ files: [file("modified")] }), merged)).toBe("changes");
    expect(deriveState(git({ unpushed: 1 }), merged)).toBe("pushed");
  });
});
