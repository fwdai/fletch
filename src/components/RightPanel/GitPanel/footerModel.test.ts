import { describe, expect, it } from "vitest";
import type { GitState, PrChecks, PrComment, PrState } from "@/api";
import { buildFooter, type FooterInput } from "./footerModel";

function git(over: Partial<GitState> = {}): GitState {
  return {
    branch: "feat",
    parent_branch: "main",
    ahead: 0,
    behind: 0,
    unpushed: 0,
    files: [],
    additions: 0,
    deletions: 0,
    has_origin: true,
    ...over,
  };
}
const file = (kind: GitState["files"][number]["kind"] = "modified") => ({
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
function checks(over: Partial<PrChecks> = {}): PrChecks {
  return {
    merge_state: "clean",
    rollup: "passing",
    total: 1,
    passed: 1,
    failed: 0,
    pending: 0,
    required_failing: [],
    runs: [],
    ...over,
  };
}
const thread = (we_replied_last: boolean) =>
  ({ url: "u", author: "a", is_bot: true, body: "b", replies: 0, we_replied_last }) as PrComment;

function footer(over: Partial<FooterInput>) {
  return buildFooter({
    git: git(),
    pr: null,
    checks: null,
    mergeState: null,
    comments: null,
    base: "main",
    commitMode: "agent-commit-pr",
    customActive: false,
    githubConnected: true,
    hasOrigin: true,
    ...over,
  });
}
const keys = (f: ReturnType<typeof buildFooter>) =>
  f.groups.flatMap((g) => g.items.map((i) => i.key));
const group = (f: ReturnType<typeof buildFooter>, title: string | undefined) =>
  f.groups.find((g) => g.title === title);

describe("buildFooter · primary follows the ladder", () => {
  it("loads before the first git read, with no menu to run blind", () => {
    const f = footer({ git: null });
    expect(f.primary.key).toBe("loading");
    expect(f.groups).toEqual([]);
  });

  it("commits uncommitted work in the sticky mode", () => {
    expect(footer({ git: git({ files: [file()] }) }).primary.key).toBe("agent-commit-pr");
    expect(footer({ git: git({ files: [file()] }), commitMode: "agent-commit" }).primary.key).toBe(
      "agent-commit",
    );
  });

  it("degrades Commit & open PR to Commit & push once a PR is open", () => {
    const f = footer({ git: git({ files: [file()] }), pr: pr() });
    expect(f.primary.key).toBe("agent-commit-push");
    // The radio group still marks the user's pick, on the row it became.
    const mode = group(f, "Commit mode");
    expect(mode?.radio).toBe(true);
    expect(mode?.items.map((i) => i.key)).not.toContain("agent-commit-pr");
    expect(mode?.items.find((i) => i.active)?.key).toBe("agent-commit-push");
  });

  it("keeps every commit mode in the radio group, the primary included", () => {
    const f = footer({ git: git({ files: [file()] }) });
    const mode = group(f, "Commit mode");
    expect(mode?.items.map((i) => i.key)).toEqual([
      "agent-commit",
      "agent-commit-push",
      "agent-commit-pr",
    ]);
    expect(mode?.items.find((i) => i.active)?.key).toBe("agent-commit-pr");
  });

  it("a typed message commits directly and drops the mode group", () => {
    const f = footer({ git: git({ files: [file()] }), customActive: true });
    expect(f.primary.key).toBe("commit-direct");
    expect(group(f, "Commit mode")).toBeUndefined();
    expect(keys(f)).toContain("commit-pr-direct");
  });

  it("resolves conflicts before anything else, with abort behind a second click", () => {
    const f = footer({ git: git({ files: [file("conflicted")] }), pr: pr() });
    expect(f.primary.key).toBe("agent-resolve");
    const abort = f.groups.flatMap((g) => g.items).find((i) => i.key === "abort");
    expect(abort?.confirm).toMatch(/click again/i);
    expect(keys(f)).not.toContain("stash");
  });

  it("pushes unpushed commits even with a PR open — the seam the old footer had", () => {
    const f = footer({
      git: git({ ahead: 3, unpushed: 1 }),
      pr: pr(),
      checks: checks(),
      mergeState: "clean",
    });
    expect(f.primary.key).toBe("push");
    expect(f.primary.label).toBe("Push 1 commit");
    // Merge stays one click away, enabled: the gate really is open.
    expect(group(f, "Pull request")?.items).toContainEqual(
      expect.objectContaining({ key: "merge", disabled: false }),
    );
  });

  it("opens a PR for pushed commits, with the auto-fill PR as the alternate", () => {
    const f = footer({ git: git({ ahead: 2 }) });
    expect(f.primary.key).toBe("agent-open-pr");
    expect(keys(f)).toContain("open-pr");
  });

  it("opens a follow-up PR after a merge", () => {
    const f = footer({ git: git({ ahead: 4, unpushed: 1 }), pr: pr({ state: "merged" }) });
    expect(f.primary).toMatchObject({ key: "agent-open-pr", label: "Open follow-up PR" });
  });

  it("merges a green PR, in green", () => {
    const f = footer({
      git: git({ ahead: 1 }),
      pr: pr(),
      checks: checks(),
      mergeState: "clean",
      comments: { unresolved: [] },
    });
    expect(f.primary).toMatchObject({ key: "merge", tone: "success" });
    expect(keys(f)).not.toContain("merge");
  });

  it("offers to fix failing checks, with Merge still enabled when GitHub allows it", () => {
    const c = checks({
      merge_state: "unstable",
      rollup: "failing",
      failed: 1,
      required_failing: ["test"],
    });
    const f = footer({ git: git({ ahead: 1 }), pr: pr(), checks: c, mergeState: "unstable" });
    expect(f.primary.key).toBe("agent-fix");
    expect(group(f, "Pull request")?.items).toContainEqual(
      expect.objectContaining({ key: "merge", disabled: false }),
    );
  });

  it("updates a branch that is behind or conflicting", () => {
    for (const ms of ["behind", "dirty"] as const) {
      const f = footer({
        git: git({ ahead: 1 }),
        pr: pr(),
        checks: checks({ merge_state: ms }),
        mergeState: ms,
      });
      expect(f.primary.key).toBe("agent-update-branch");
    }
  });

  it("hands review threads waiting on us to the agent", () => {
    const f = footer({
      git: git({ ahead: 1 }),
      pr: pr(),
      checks: checks(),
      mergeState: "clean",
      comments: { unresolved: [thread(false)] },
    });
    expect(f.primary.key).toBe("agent-resolve-comments");
  });

  it("sends human-owned gates to GitHub", () => {
    const review = footer({
      git: git({ ahead: 1 }),
      pr: pr(),
      checks: checks({ merge_state: "blocked" }),
      mergeState: "blocked",
    });
    expect(review.primary.key).toBe("view-pr");
    const draft = footer({
      git: git({ ahead: 1 }),
      pr: pr(),
      checks: checks({ merge_state: "draft" }),
      mergeState: "draft",
    });
    expect(draft.primary).toMatchObject({ key: "view-pr", tone: "ghost" });
    const closed = footer({ git: git({ ahead: 1 }), pr: pr({ state: "closed" }) });
    expect(closed.primary).toMatchObject({ key: "agent-open-pr", label: "Open new PR" });
  });

  it("stays quiet on a clean checkout: pull, or rebase when behind", () => {
    expect(footer({}).primary).toMatchObject({ key: "pull", tone: "ghost" });
    expect(footer({ git: git({ behind: 2 }) }).primary).toMatchObject({
      key: "rebase",
      tone: "ghost",
    });
    expect(footer({ git: git({ ahead: 3 }), pr: pr({ state: "merged" }) }).primary.key).toBe(
      "pull",
    );
  });
});

describe("buildFooter · menu", () => {
  it("never repeats the primary and drops a group that only held it", () => {
    const f = footer({});
    expect(keys(f)).not.toContain("pull");
    expect(keys(f)).toContain("rebase");
  });

  it("puts the destructive actions last, behind a second click", () => {
    const f = footer({ git: git({ files: [file(), file()] }) });
    const last = f.groups[f.groups.length - 1];
    expect(last.title).toBeUndefined();
    expect(last.items.map((i) => i.key)).toEqual(["stash", "discard"]);
    expect(last.items[1]).toMatchObject({ danger: true, confirm: "Discard 2 files? Click again" });
  });

  it("offline: commits locally and offers Connect GitHub instead of push or PR", () => {
    const f = footer({ git: git({ files: [file()] }), githubConnected: false });
    expect(f.primary.key).toBe("agent-commit");
    expect(group(f, "Commit mode")).toBeUndefined();
    expect(keys(f)).toContain("connect-github");
    expect(keys(f)).not.toContain("agent-commit-push");
    expect(keys(f)).toContain("stash");
  });

  it("local-only: offers both publish visibilities", () => {
    const f = footer({ git: git({ ahead: 1 }), hasOrigin: false });
    expect(f.primary.key).toBe("publish");
    expect(keys(f)).toContain("publish-public");
    expect(keys(f)).not.toContain("open-pr");
  });
});
