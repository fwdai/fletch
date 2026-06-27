import { describe, expect, it } from "vitest";
import type { GitState, PrState } from "../../api";
import { deriveState, primaryFor, secondaryFor } from "./primaryActions";

const base = { prNumber: 7, base: "main" };

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
  return { number: 7, url: "https://x", state: "open", title: "t", mergeable: true, ...over };
}

describe("deriveState", () => {
  it("uncommitted changes take precedence over an open PR", () => {
    expect(deriveState(git({ files: [file("modified")] }), pr())).toBe("changes");
    expect(deriveState(git(), pr())).toBe("pr-open");
  });

  it("conflicts and merged still take precedence over changes", () => {
    expect(deriveState(git({ files: [file("conflicted")] }), pr())).toBe("conflicts");
    expect(deriveState(git({ files: [file("modified")] }), pr({ state: "merged" }))).toBe("merged");
  });
});

describe("changes-state sticky commit action", () => {
  it("defaults to Commit & open PR", () => {
    const p = primaryFor("changes", { files: 2 });
    expect(p.key).toBe("agent-commit-pr");
  });

  it("honors the persisted selection", () => {
    expect(primaryFor("changes", { files: 2, commitAction: "agent-commit" }).key).toBe(
      "agent-commit",
    );
    expect(primaryFor("changes", { files: 2, commitAction: "agent-commit-push" }).key).toBe(
      "agent-commit-push",
    );
  });

  it("remaps Commit & open PR to Commit & push when a PR is already open", () => {
    const p = primaryFor("changes", { files: 2, commitAction: "agent-commit-pr", prOpen: true });
    expect(p.key).toBe("agent-commit-push");
  });

  it("menu offers the other commit modes, and Merge PR when a PR is open", () => {
    const closedKeys = secondaryFor("changes", { files: 2 }).map((s) => s.key);
    expect(closedKeys).toContain("agent-commit");
    expect(closedKeys).toContain("agent-commit-push");
    expect(closedKeys).not.toContain("merge");
    const openKeys = secondaryFor("changes", { files: 2, prOpen: true }).map((s) => s.key);
    expect(openKeys).toContain("merge");
    expect(openKeys).not.toContain("agent-commit-pr"); // meaningless with a PR open
  });
});

describe("pr-open primary by merge_state (§7)", () => {
  it("clean → green Merge PR", () => {
    const p = primaryFor("pr-open", { ...base, mergeState: "clean" });
    expect(p.key).toBe("merge");
    expect(p.tone).toBe("success");
    expect(p.statusKind).toBe("ready");
    expect(p.statusLabel).toContain("ready to merge");
  });

  it("unstable → Merge PR enabled, warn status", () => {
    const p = primaryFor("pr-open", { ...base, mergeState: "unstable", checksFailed: 1 });
    expect(p.key).toBe("merge");
    expect(p.tone).toBeUndefined();
    expect(p.statusKind).toBe("warn");
  });

  it("blocked with failing checks → Fix with agent", () => {
    const p = primaryFor("pr-open", { ...base, mergeState: "blocked", checksFailed: 2 });
    expect(p.key).toBe("agent-fix");
    expect(p.statusKind).toBe("attention");
    expect(p.statusLabel).toContain("2 checks failing");
  });

  it("blocked with no failing checks → review gate, View on GitHub", () => {
    const p = primaryFor("pr-open", { ...base, mergeState: "blocked", checksFailed: 0 });
    expect(p.key).toBe("view-pr");
    expect(p.statusLabel).toContain("review");
  });

  it("behind / dirty → Update branch with agent", () => {
    for (const ms of ["behind", "dirty"] as const) {
      const p = primaryFor("pr-open", { ...base, mergeState: ms });
      expect(p.key).toBe("agent-update-branch");
      expect(p.statusKind).toBe("attention");
    }
  });

  it("unknown / has_hooks → Merge with checking status", () => {
    for (const ms of ["unknown", "has_hooks"] as const) {
      const p = primaryFor("pr-open", { ...base, mergeState: ms });
      expect(p.key).toBe("merge");
      expect(p.statusLabel).toContain("checking");
    }
  });

  it("no checks data falls back to mergeable-only behavior", () => {
    const ok = primaryFor("pr-open", { ...base, mergeable: true });
    expect(ok.key).toBe("merge");
    expect(ok.statusLabel).toContain("no conflicts");
    const blocked = primaryFor("pr-open", { ...base, mergeable: false });
    expect(blocked.key).toBe("merge");
    expect(blocked.statusKind).toBe("attention");
  });
});

describe("pr-open secondary menu", () => {
  it("offers merge as an alternate and update-branch when behind", () => {
    const keys = secondaryFor("pr-open", { ...base, mergeState: "behind" }).map((s) => s.key);
    expect(keys).toContain("merge");
    expect(keys).toContain("agent-update-branch");
    // View on GitHub is a convenience link (a chip next to the status), not
    // an action — it doesn't belong in the action menu.
    expect(keys).not.toContain("view-pr");
  });

  it("offers agent-fix when checks are failing", () => {
    const keys = secondaryFor("pr-open", { ...base, mergeState: "unstable", checksFailed: 1 }).map(
      (s) => s.key,
    );
    expect(keys).toContain("agent-fix");
  });

  it("delegated open-pr is the pushed-state primary", () => {
    expect(primaryFor("pushed", { ahead: 2 }).key).toBe("agent-open-pr");
    const keys = secondaryFor("pushed", {}).map((s) => s.key);
    expect(keys).toContain("open-pr"); // direct auto-fill alternate
  });
});
