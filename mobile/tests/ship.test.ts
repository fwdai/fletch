// The Ship tab's view (src/screens/Agent/ShipTab/derive.ts) over the desktop
// Git panel's ladder and footer, its dispatch table (ShipTab/actions.ts), and
// its autopilot line (ShipTab/autopilot.ts). The in-flight playbook is the
// host's (`delegation:changed`), covered with the store in store.test.ts.

import type { AutopilotCheckout, AutopilotCycle, GitState } from "@desktop/api/types/git";
import type { CheckRun, PrChecks, PrComment, PrComments, PrState } from "@desktop/api/types/pr";
import { playbookFor } from "@desktop/components/RightPanel/GitPanel/actionPlan";
import { buildFooter, type FooterInput } from "@desktop/components/RightPanel/GitPanel/footerModel";
import { buildLadder } from "@desktop/components/RightPanel/GitPanel/ladderModel";
import { delegationLabel } from "@desktop/delegation";
import { GATES } from "@desktop/store/gates";
import { describe, expect, it } from "vitest";
import { type ShipFacts, shipOp, shipRefusal } from "../src/screens/Agent/ShipTab/actions";
import { describeAutopilot } from "../src/screens/Agent/ShipTab/autopilot";
import { type ShipInput, shipView, stripKind } from "../src/screens/Agent/ShipTab/derive";
import { agentCheckouts, autopilotFromSnapshot } from "../src/store/autopilot";

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

/** A required check failing, GitHub still willing to merge. */
const failing = checks({
  merge_state: "unstable",
  rollup: "failing",
  failed: 1,
  required_failing: ["test"],
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

const input = (over: Partial<ShipInput> = {}): ShipInput => ({
  git: git(),
  pr: null,
  checks: null,
  comments: null,
  base: "main",
  delegation: null,
  canMerge: true,
  ...over,
});
const ship = (over: Partial<ShipInput> = {}) => shipView(input(over));
const keys = (over: Partial<ShipInput> = {}) =>
  ship(over).footer.groups.flatMap((g) => g.items.map((i) => i.key));
const item = (over: Partial<ShipInput>, key: string) =>
  ship(over)
    .footer.groups.flatMap((g) => g.items)
    .find((i) => i.key === key);

/** What the desktop panel shows for the same facts: its primary's label and
 *  its current row's summary. */
function desktop(over: Partial<ShipInput>) {
  const i = input(over);
  const checks = i.checks ?? null;
  const mergeState = checks?.merge_state ?? null;
  const facts: FooterInput = {
    git: i.git,
    pr: i.pr,
    checks,
    mergeState,
    comments: i.comments,
    base: i.base,
    commitMode: "agent-commit-pr",
    customActive: false,
    githubConnected: true,
    hasOrigin: true,
  };
  return {
    primary: buildFooter(facts).primary.label,
    current: buildLadder(facts).find((r) => r.current)?.summary,
  };
}

describe("shipView · the desktop's models", () => {
  it("says what the desktop panel says, state for state", () => {
    const states: Partial<ShipInput>[] = [
      { git: git({ files: [file()] }) },
      { git: git({ files: [file(), file()] }), pr: pr(), checks: checks() },
      { git: git({ ahead: 2 }) },
      { git: git({ ahead: 3, unpushed: 1 }), pr: pr(), checks: checks() },
      { git: git({ files: [file("conflicted")] }), pr: pr(), checks: checks() },
      { git: git({ ahead: 1 }), pr: pr(), checks: checks({ merge_state: "behind" }) },
      { git: git({ ahead: 1 }), pr: pr(), checks: failing },
      { git: git({ ahead: 1 }), pr: pr(), checks: checks(), comments: threads(thread()) },
      { git: git({ ahead: 1 }), pr: pr(), checks: checks(), comments: threads() },
      { git: git({ ahead: 4, unpushed: 1 }), pr: pr({ state: "merged" }) },
    ];
    for (const over of states) {
      const v = ship(over);
      const d = desktop(over);
      expect(v.footer.primary?.label).toBe(d.primary);
      expect(v.strip.text).toBe(d.current);
    }
  });

  it("tints the strip off the current row and shows the diff only for changes", () => {
    expect(ship({ git: git({ files: [file()] }) }).strip).toEqual({
      kind: "changes",
      text: "1 file",
      diff: true,
      prLink: false,
    });
    const merge = ship({ git: git({ ahead: 1 }), pr: pr(), checks: checks(), comments: threads() });
    expect(merge.strip).toEqual({
      kind: "ready",
      text: "#7 · ready to merge",
      diff: false,
      prLink: true,
    });
    expect(merge.footer.primary).toMatchObject({ key: "merge", tone: "success" });
  });

  it("reads an open PR whose checks were never read as still checking", () => {
    // `prChecks` has no entry yet: the gate is "unknown", as `useGitPanelData` has it.
    const reading = ship({ git: git({ ahead: 1 }), pr: pr(), checks: undefined });
    expect(reading.ladder.find((r) => r.key === "checks")).toMatchObject({
      mark: "running",
      summary: "checking…",
    });
    // A confirmed null is no data, not a read in flight.
    const none = ship({ git: git({ ahead: 1 }), pr: pr(), checks: null });
    expect(none.ladder.find((r) => r.key === "checks")).toMatchObject({
      mark: "idle",
      summary: "unavailable",
    });
  });

  it("says there is nothing to ship for a clean checkout with no PR, with nothing to press", () => {
    const v = ship();
    expect(v.strip).toEqual({ kind: "clean", text: "Nothing to ship yet", sub: "← main" });
    // The desktop's quiet Pull: the phone has no pull to run.
    expect(v.footer).toEqual({ primary: null, groups: [], mainReason: null });
  });
});

describe("shipView · the phone's own strips", () => {
  it("is neutral with nothing to press while the first read is in flight", () => {
    const v = ship({ git: null });
    expect(v.strip).toEqual({ kind: "neutral", text: "Loading…" });
    expect(v.ladder).toEqual([]);
    expect(v.footer).toEqual({ primary: null, groups: [], mainReason: null });
  });

  it("says Git paused when the checkout's config blocks git, whatever else it reads", () => {
    const v = ship({ git: git({ files: [file()], blocked_config: ["core.hooksPath"] }), pr: pr() });
    expect(v.strip).toEqual({ kind: "att", text: "Git paused", sub: "blocking settings" });
    expect(v.ladder).toEqual([]);
    expect(v.footer.primary).toBeNull();
  });

  it("shows the playbook in flight and holds every action quietly", () => {
    const v = ship({ git: git({ files: [file()] }), pr: pr(), delegation: "commit-push" });
    expect(v.strip).toEqual({
      kind: "working",
      text: delegationLabel("commit-push"),
      prLink: true,
    });
    expect(v.footer.primary).toMatchObject({ key: "agent-commit-push", disabled: true });
    expect(v.footer.mainReason).toBeNull();
  });
});

describe("shipView · what the phone can run", () => {
  it("drops the keys the phone has no dispatch for", () => {
    // Conflicts: the desktop also offers pull, rebase and abort/discard.
    expect(keys({ git: git({ files: [file("conflicted")] }) })).toEqual([]);
    // No origin: the desktop's primary is Publish, with both visibilities.
    const local = ship({ git: git({ ahead: 1, has_origin: false }) });
    expect(local.footer.primary).toBeNull();
    expect(keys({ git: git({ ahead: 1, has_origin: false }) })).toEqual([]);
  });

  it("puts the manual sheet behind a commit-family primary, once", () => {
    expect(keys({ git: git({ files: [file()] }) })).toEqual(["manual"]);
    // Pushed with no PR: the desktop's auto-fill PR is the PR sheet here, so it
    // folds into the one manual row.
    const pushed = ship({ git: git({ ahead: 2 }) });
    expect(pushed.footer.primary?.key).toBe("agent-open-pr");
    expect(keys({ git: git({ ahead: 2 }) })).toEqual(["manual"]);
    // A PR remedy is not a commit: no manual row.
    expect(keys({ git: git({ ahead: 1 }), pr: pr(), checks: failing })).toEqual(["merge"]);
  });

  it("offers Archive for a merged PR, gated on work only this checkout holds", () => {
    const landed = { git: git({ ahead: 3 }), pr: pr({ state: "merged" }) };
    expect(ship(landed).footer.primary).toBeNull();
    expect(ship(landed).footer.groups).toEqual([
      {
        title: "Workspace",
        items: [{ key: "archive", label: "Archive workspace", icon: "archive" }],
      },
    ]);
    const since = { git: git({ ahead: 3, files: [file()] }), pr: pr({ state: "merged" }) };
    expect(item(since, "archive")).toMatchObject({
      disabled: true,
      reason: "Work since the merge is only here",
    });
  });

  it("disables Merge with the reason when the host or GitHub's gate won't take it", () => {
    const open = { git: git({ ahead: 1 }), pr: pr(), checks: failing };
    expect(item(open, "merge")).not.toHaveProperty("disabled", true);
    expect(item({ ...open, canMerge: false }, "merge")).toMatchObject({
      disabled: true,
      reason: GATES.mergePr.reason,
    });
    const shut = checks({
      merge_state: "blocked",
      rollup: "failing",
      failed: 1,
      required_failing: ["test"],
    });
    expect(item({ ...open, checks: shut }, "merge")).toMatchObject({
      disabled: true,
      reason: expect.stringMatching(/merge gate/),
    });
    // The primary carries the reason too.
    const green = { git: git({ ahead: 1 }), pr: pr(), checks: checks(), comments: threads() };
    expect(ship({ ...green, canMerge: false }).footer).toMatchObject({
      primary: { key: "merge", disabled: true },
      mainReason: GATES.mergePr.reason,
    });
  });
});

describe("stripKind", () => {
  it("tints by the row's attention colour, else by its mark", () => {
    expect(stripKind({ mark: "attention", tone: "warn" })).toBe("changes");
    expect(stripKind({ mark: "attention", tone: "att" })).toBe("att");
    expect(stripKind({ mark: "attention", tone: "danger" })).toBe("att");
    expect(stripKind({ mark: "attention", tone: "info" })).toBe("info");
    expect(stripKind({ mark: "attention", tone: "neutral" })).toBe("neutral");
    expect(stripKind({ mark: "done", tone: "merged" })).toBe("merged");
    expect(stripKind({ mark: "done" })).toBe("ready");
    expect(stripKind({ mark: "running" })).toBe("info");
    expect(stripKind({ mark: "idle" })).toBe("neutral");
  });
});

describe("shipOp", () => {
  const facts: ShipFacts = {
    base: "main",
    pr: pr(),
    checks: checks({ required_failing: ["lint", "test"] }),
    comments: threads(thread(), thread({ id: "t2" }), thread({ id: "t3", we_replied_last: true })),
  };

  it("hands the agent the desktop's plan for every delegated key, push included", () => {
    for (const key of [
      "agent-commit",
      "agent-commit-push",
      "agent-commit-pr",
      "agent-open-pr",
      "agent-resolve",
      "agent-update-branch",
      "agent-fix",
      "agent-resolve-comments",
      "push",
    ]) {
      const playbook = playbookFor(key, facts);
      expect(playbook).not.toBeNull();
      expect(shipOp(key, facts)).toEqual({ kind: "delegate", playbook });
    }
  });

  it("maps the rest to the host, GitHub or the PR sheet", () => {
    expect(shipOp("merge", facts)).toEqual({ kind: "merge" });
    expect(shipOp("archive", facts)).toEqual({ kind: "archive" });
    expect(shipOp("view-pr", facts)).toEqual({ kind: "github", url: pr().url });
    for (const key of ["open-pr", "commit-direct", "commit-pr-direct", "manual"]) {
      expect(shipOp(key, facts)).toEqual({ kind: "manual" });
    }
  });

  it("has nothing for what the phone can't run, and the gate says so", () => {
    for (const key of [
      "connect-github",
      "publish",
      "publish-public",
      "pull",
      "rebase",
      "stash",
      "discard",
      "abort",
      "delete-branch",
      "focus-pr",
      "clear-config",
      "loading",
    ]) {
      expect(shipOp(key, facts)).toBeNull();
      expect(shipRefusal(key, facts, true)).not.toBeNull();
    }
    expect(shipRefusal("merge", facts, false)).toBe(GATES.mergePr.reason);
    expect(shipRefusal("merge", facts, true)).toBeNull();
    expect(shipRefusal("agent-commit-pr", facts, false)).toBeNull();
  });
});

describe("describeAutopilot", () => {
  const row = (over: Partial<AutopilotCheckout> = {}): AutopilotCheckout => ({
    agent_id: "a",
    subdir: null,
    project_id: "p",
    enrolled: true,
    paused: false,
    project_enabled: true,
    cycle: null,
    ...over,
  });
  const cycle = (over: Partial<AutopilotCycle> = {}): AutopilotCycle => ({
    rung: "fix-checks",
    attempt: 1,
    phase: "working",
    since: 1,
    ...over,
  });

  it("says nothing for an agent the host reported no rows for", () => {
    expect(describeAutopilot([], true)).toBeNull();
  });

  it("is on by default, with the agent's switch when the host takes it", () => {
    expect(describeAutopilot([row()], true)).toEqual({
      text: "Autopilot on",
      on: true,
      switchable: true,
      busy: false,
    });
    // A Control pairing has no `autopilot_set`: the line stays, the switch goes.
    expect(describeAutopilot([row()], false)?.switchable).toBe(false);
  });

  it("names the rung as the desktop does, and the try from the second on", () => {
    expect(describeAutopilot([row({ cycle: cycle() })], true)?.text).toBe(
      "Autopilot: working on the failing checks",
    );
    expect(
      describeAutopilot([row({ cycle: cycle({ rung: "resolve", attempt: 2 }) })], true),
    ).toMatchObject({ text: "Autopilot: working on the conflicts, try 2", busy: true });
    expect(
      describeAutopilot(
        [row({ cycle: cycle({ phase: "awaiting-evidence", rung: "resolve-comments" }) })],
        true,
      )?.text,
    ).toBe("Autopilot: checking its work on the review comments");
  });

  it("speaks for the first checkout with a cycle, the primary first", () => {
    const map = autopilotFromSnapshot({
      checkouts: [
        row({ subdir: "api", cycle: cycle({ rung: "update-branch" }) }),
        row({ subdir: "web", cycle: cycle() }),
        row(),
      ],
      disabled_projects: [],
      paused_agents: [],
    });
    const rows = agentCheckouts(map, "a");
    expect(rows.map((r) => r.subdir)).toEqual([null, "api", "web"]);
    expect(describeAutopilot(rows, true)?.text).toBe(
      "Autopilot: working on the branch update (api)",
    );
    // The primary's own cycle wins over a secondary's.
    const primaryBusy = agentCheckouts({ ...map, a: row({ cycle: cycle({ attempt: 3 }) }) }, "a");
    expect(describeAutopilot(primaryBusy, true)?.text).toBe(
      "Autopilot: working on the failing checks, try 3",
    );
  });

  it("says when the agent is paused, and when the project's switch is off", () => {
    expect(describeAutopilot([row({ paused: true, enrolled: false })], true)).toEqual({
      text: "Autopilot paused",
      on: false,
      switchable: true,
      busy: false,
    });
    // The phone flips only the pause, which means nothing with the project off.
    expect(describeAutopilot([row({ project_enabled: false, enrolled: false })], true)).toEqual({
      text: "Autopilot is off for this project",
      on: false,
      switchable: false,
      busy: false,
    });
  });
});
