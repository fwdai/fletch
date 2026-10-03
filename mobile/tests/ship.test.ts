// The Ship tab's derivation (src/screens/Agent/ShipTab/derive.ts) over the
// desktop's ladder, and the in-flight playbook read off the chat log.

import type { GitState } from "@desktop/api/types/git";
import type { CheckRun, PrChecks, PrComment, PrComments, PrState } from "@desktop/api/types/pr";
import { delegationLabel } from "@desktop/delegation";
import type { ReadinessInput } from "@desktop/readiness";
import { describe, expect, it } from "vitest";
import type { ChatItem } from "../src/adapters";
import { activeDelegation, appActionName } from "../src/screens/Agent/ShipTab/delegation";
import { describeShip, type ShipExtra } from "../src/screens/Agent/ShipTab/derive";

const git = (over: Partial<GitState> = {}): GitState => ({
  branch: "feat/x",
  parent_branch: "main",
  ahead: 0,
  behind: 0,
  unpushed: 0,
  files: [],
  additions: 0,
  deletions: 0,
  has_origin: true,
  ...over,
});

const file = (kind: GitState["files"][number]["kind"] = "modified") => ({
  path: "a.ts",
  kind,
  staged: false,
  additions: 1,
  deletions: 0,
});

const pr = (over: Partial<PrState> = {}): PrState => ({
  number: 7,
  url: "https://github.com/o/r/pull/7",
  state: "open",
  title: "x",
  mergeable: "mergeable",
  ...over,
});

const run = (over: Partial<CheckRun> = {}): CheckRun => ({
  name: "build",
  status: "completed",
  conclusion: "success",
  required: true,
  url: null,
  started_at: null,
  completed_at: null,
  ...over,
});

const checks = (over: Partial<PrChecks> = {}): PrChecks => ({
  merge_state: "clean",
  rollup: "passing",
  total: 1,
  passed: 1,
  failed: 0,
  pending: 0,
  required_failing: [],
  runs: [run()],
  ...over,
});

const thread = (over: Partial<PrComment> = {}): PrComment => ({
  id: "t1",
  author: "reviewer",
  is_bot: false,
  body: "nit",
  path: null,
  line: null,
  url: "https://github.com/o/r/pull/7#discussion_r1",
  replies: 0,
  we_replied_last: false,
  ...over,
});

const threads = (...items: PrComment[]): PrComments => ({ unresolved: items });

const ship = (input: Partial<ReadinessInput> = {}, extra: Partial<ShipExtra> = {}) =>
  describeShip(
    { git: git(), pr: null, checks: null, comments: null, ...input },
    { base: "main", commitMode: "commit-pr" },
    { delegation: null, canMerge: true, ...extra },
  );

const keys = (...args: Parameters<typeof ship>) => ship(...args).more.map((a) => a.key);

describe("describeShip", () => {
  it("is neutral with nothing to press while the first read is in flight", () => {
    const v = ship({ git: null });
    expect(v.rung).toEqual({ do: "wait", why: "unknown-state" });
    expect(v.strip).toEqual({ kind: "neutral", text: "Loading…" });
    expect(v.primary).toBeNull();
    expect(v.more).toEqual([]);
  });

  it("says there is nothing to ship for a clean checkout with no PR", () => {
    const v = ship();
    expect(v.strip).toEqual({ kind: "clean", text: "Nothing to ship yet", sub: "← main" });
    expect(v.primary).toBeNull();
    expect(v.more).toEqual([]);
  });

  it("leads with Commit & open PR for uncommitted work, the manual sheet behind it", () => {
    const v = ship({ git: git({ files: [file()], additions: 3 }) });
    expect(v.strip).toEqual({
      kind: "changes",
      text: "1 uncommitted file",
      diff: true,
      prLink: false,
    });
    expect(v.primary).toEqual({
      key: "commit-pr",
      label: "Commit & open PR",
      kind: "delegate",
      action: "commit-pr",
      params: { base: "main" },
      delegation: "commit-pr",
    });
    expect(v.more).toEqual([
      { key: "manual", label: "Write the message yourself", kind: "manual" },
    ]);
  });

  it("degrades to Commit & push once a PR is open, and keeps GitHub in reach", () => {
    const v = ship({ git: git({ files: [file(), file()] }), pr: pr() });
    expect(v.strip.text).toBe("2 uncommitted files");
    expect(v.strip.prLink).toBe(true);
    expect(v.primary).toMatchObject({ key: "commit-push", label: "Commit & push to #7" });
    expect(v.more.map((a) => a.key)).toEqual(["manual", "github"]);
  });

  it("opens a PR for commits ahead of base, pushes to an existing one", () => {
    const open = ship({ git: git({ ahead: 2 }) });
    expect(open.strip).toEqual({ kind: "info", text: "Pushed, no PR yet", prLink: false });
    expect(open.primary).toMatchObject({
      key: "open-pr",
      label: "Open PR",
      params: { base: "main" },
    });
    expect(open.more.map((a) => a.key)).toEqual(["manual"]);

    const push = ship({ git: git({ unpushed: 1 }), pr: pr() });
    expect(push.strip).toEqual({ kind: "info", text: "1 commit not pushed", prLink: true });
    expect(push.primary).toMatchObject({ key: "push", label: "Push to #7", action: "push" });
    expect(push.primary).not.toHaveProperty("params");
  });

  it("resolves conflicts before anything else, with no manual alternative", () => {
    const v = ship({
      git: git({ files: [file("conflicted"), file()], unpushed: 2 }),
      pr: pr(),
      checks: checks(),
      comments: threads(thread()),
    });
    expect(v.strip).toEqual({ kind: "att", text: "Conflicts in 1 file", prLink: true });
    expect(v.primary).toEqual({
      key: "resolve",
      label: "Resolve conflicts",
      kind: "delegate",
      action: "resolve-conflicts",
      delegation: "resolve",
    });
    // The gate is green, so Merge stays reachable — but never the PR sheet.
    expect(v.more.map((a) => a.key)).toEqual(["merge", "github"]);
  });

  it("updates the branch when behind or conflicting with the base, naming which", () => {
    const behind = ship({ pr: pr(), checks: checks({ merge_state: "behind" }) });
    expect(behind.strip).toEqual({ kind: "att", text: "Behind main", prLink: true });
    expect(behind.primary).toMatchObject({
      key: "update-branch",
      label: "Update branch",
      params: { base: "main" },
    });
    expect(behind.more.map((a) => a.key)).toEqual(["github"]);

    const dirty = ship({ pr: pr(), checks: checks({ merge_state: "dirty" }) });
    expect(dirty.strip.text).toBe("Conflicts with main");
    expect(ship({ pr: pr({ mergeable: "conflicting" }) }).strip.text).toBe("Conflicts with main");
  });

  it("fixes failing checks, naming them, while Merge stays in More when GitHub would take it", () => {
    const v = ship({
      pr: pr(),
      checks: checks({ merge_state: "unstable", failed: 2, required_failing: ["lint", "test"] }),
    });
    expect(v.strip).toEqual({ kind: "att", text: "2 checks failing", prLink: true });
    expect(v.primary).toMatchObject({
      key: "fix-checks",
      label: "Fix failing checks",
      params: { failing: "lint, test" },
    });
    expect(v.more.map((a) => a.key)).toEqual(["merge", "github"]);
    // A shut gate takes Merge out of the sheet.
    expect(
      keys({ pr: pr(), checks: checks({ merge_state: "blocked", required_failing: ["test"] }) }),
    ).toEqual(["github"]);
  });

  it("resolves the review threads awaiting us, not the ones we pushed back on", () => {
    const v = ship({
      pr: pr(),
      checks: checks(),
      comments: threads(
        thread(),
        thread({ id: "t2" }),
        thread({ id: "t3", we_replied_last: true }),
      ),
    });
    expect(v.strip).toEqual({ kind: "att", text: "2 review comments waiting", prLink: true });
    expect(v.primary).toMatchObject({
      key: "resolve-comments",
      label: "Resolve 2 review comments",
      params: { count: "2" },
    });
    expect(v.more.map((a) => a.key)).toEqual(["merge", "github"]);
    expect(ship({ pr: pr(), checks: checks(), comments: threads(thread()) }).primary?.label).toBe(
      "Resolve 1 review comment",
    );
  });

  it("escalates a disputed thread: nothing for the agent, GitHub for the human", () => {
    const v = ship({
      pr: pr(),
      checks: checks(),
      comments: threads(thread({ we_replied_last: true })),
    });
    expect(v.rung).toMatchObject({ do: "escalate", blocker: { kind: "review-disputed" } });
    expect(v.strip).toEqual({ kind: "info", text: "1 thread waiting on a reply", prLink: true });
    expect(v.primary).toMatchObject({ key: "github", url: "https://github.com/o/r/pull/7" });
    // The gate is open, so Merge is still offered — a person may overrule.
    expect(v.more.map((a) => a.key)).toEqual(["merge"]);
  });

  it("offers Merge as the primary when the gate is open and the host can", () => {
    const v = ship({ pr: pr(), checks: checks() });
    expect(v.rung).toEqual({ do: "merge" });
    expect(v.strip).toEqual({ kind: "ready", text: "Ready to merge", prLink: true });
    expect(v.primary).toEqual({ key: "merge", label: "Merge PR #7", kind: "merge" });
    expect(v.more.map((a) => a.key)).toEqual(["github"]);
  });

  it("falls back to GitHub when the host cannot merge, without listing it twice", () => {
    const v = ship({ pr: pr(), checks: checks() }, { canMerge: false });
    expect(v.strip.text).toBe("Ready to merge");
    expect(v.primary).toMatchObject({ key: "github", label: "Open on GitHub" });
    expect(v.more).toEqual([]);
  });

  it("escalates the human-owned gates to GitHub", () => {
    const review = ship({ pr: pr(), checks: checks({ merge_state: "blocked" }) });
    expect(review.strip).toEqual({ kind: "info", text: "Waiting on a review", prLink: true });
    expect(review.primary?.kind).toBe("github");
    expect(review.more).toEqual([]);

    const draft = ship({ pr: pr(), checks: checks({ merge_state: "draft" }) });
    expect(draft.strip.text).toBe("Draft on GitHub");

    const closed = ship({ pr: pr({ state: "closed" }) });
    expect(closed.strip).toEqual({ kind: "neutral", text: "PR #7 was closed", prLink: false });
    expect(closed.primary?.kind).toBe("github");
  });

  it("waits while GitHub is still computing the gate", () => {
    const v = ship({ pr: pr({ mergeable: "unknown" }) });
    expect(v.rung).toEqual({ do: "wait", why: "gate-computing" });
    expect(v.strip).toEqual({
      kind: "info",
      text: "GitHub is computing merge status",
      prLink: true,
    });
    expect(v.primary?.kind).toBe("github");
    expect(v.more).toEqual([]);
  });

  it("reads the gate's own words when nothing blocks but the gate is not open", () => {
    // No checks data, no conflicts: not a blocker, not merge-ready either.
    const v = ship({ pr: pr() });
    expect(v.rung).toEqual({ do: "ready" });
    expect(v.strip).toEqual({ kind: "info", text: "No conflicts", prLink: true });
    expect(v.primary?.kind).toBe("github");
  });

  it("offers to archive once the PR merged with nothing since", () => {
    const v = ship({ git: git({ ahead: 3 }), pr: pr({ state: "merged" }) });
    expect(v.rung).toEqual({ do: "landed" });
    expect(v.strip).toEqual({ kind: "merged", text: "Merged into main", prLink: true });
    expect(v.primary).toEqual({ key: "archive", label: "Archive workspace", kind: "archive" });
    expect(v.more.map((a) => a.key)).toEqual(["github"]);
  });

  it("climbs the ladder again for follow-up work after a merge — a new PR, not a push", () => {
    const v = ship({ git: git({ files: [file()] }), pr: pr({ state: "merged" }) });
    expect(v.strip).toMatchObject({ kind: "changes", text: "1 uncommitted file" });
    expect(v.primary).toMatchObject({ key: "commit-pr", params: { base: "main" } });
    expect(v.more.map((a) => a.key)).toEqual(["manual", "github"]);
  });

  it("shows the playbook in flight and holds the primary, keeping More", () => {
    const v = ship({ git: git({ files: [file()] }), pr: pr() }, { delegation: "commit-push" });
    expect(v.strip).toEqual({
      kind: "working",
      text: delegationLabel("commit-push"),
      prLink: true,
    });
    expect(v.primary).toBeNull();
    expect(v.more.map((a) => a.key)).toEqual(["manual", "github"]);
  });

  it("says Git paused when the checkout's config blocks git, whatever else it reads", () => {
    const v = ship({ git: git({ files: [file()], blocked_config: ["core.hooksPath"] }), pr: pr() });
    expect(v.strip).toEqual({ kind: "att", text: "Git paused", sub: "blocking settings" });
    expect(v.primary).toBeNull();
    expect(v.more).toEqual([]);
  });
});

const user = (text: string): ChatItem => ({ kind: "user_message", text });
const queued = (text: string): ChatItem => ({ kind: "queued_message", text });
const agent = (text: string): ChatItem => ({ kind: "agent_message", text });

describe("activeDelegation", () => {
  it("is null while the agent is idle, whatever the log says", () => {
    expect(activeDelegation([user("[app-action] commit-pr")], false)).toBeNull();
    expect(activeDelegation(undefined, true)).toBeNull();
  });

  it("reads the kind off the message that opened the running turn", () => {
    const log = [
      user("hi"),
      agent("hello"),
      user('[app-action] commit-pr base="main"'),
      agent("Committing…"),
    ];
    expect(activeDelegation(log, true)).toBe("commit-pr");
  });

  it("maps the resolve-conflicts trigger onto the resolve kind", () => {
    expect(activeDelegation([user("[app-action] resolve-conflicts")], true)).toBe("resolve");
  });

  it("counts the optimistic bubble, since our own send sits there first", () => {
    expect(activeDelegation([user("hi"), queued("[app-action] push")], true)).toBe("push");
  });

  it("is null for a plain turn, and for a trigger this build does not know", () => {
    expect(
      activeDelegation([user("[app-action] commit-pr"), user("and now fix the test")], true),
    ).toBeNull();
    expect(activeDelegation([user("[app-action] deploy-prod")], true)).toBeNull();
    expect(appActionName("[app-action] ")).toBeNull();
  });
});
