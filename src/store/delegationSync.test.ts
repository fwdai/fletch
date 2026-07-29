// Delegations are keyed per *checkout* and their lifecycle runs app-wide, not in
// the Git panel. These tests pin the two bugs that motivated the move and the
// re-key:
//
//   1. A delegation dispatched at an agent the user isn't looking at was never
//      advanced at all — and for a *running* agent that meant its trigger was
//      held forever, so the agent never did the work. Mission Control's
//      `approveAgent` / `updateAll` dispatch exactly that way.
//   2. Delegations were keyed by agent, so a multi-repo agent could only ever
//      hold one, and an ack from either checkout applied to it.
//
// `planDelegationPass` is the whole sweep, so both are testable without a panel.

import { describe, expect, it, vi } from "vitest";
import { create } from "zustand";
import type { AgentStatus, GitState, PrState } from "@/api";
import { DELEGATION_GIVE_UP_GRACE_MS, type Delegation } from "@/delegation";
import { type DelegationEffect, planDelegationPass } from "./delegationSync";

vi.mock("@/api", () => ({ api: {} }));
vi.mock("@/storage/settings", () => ({ setSetting: vi.fn() }));

import { dropAgentEntries } from "@/helpers/agentLookups";
import { checkoutKey, createGitSlice, splitCheckoutKey } from "./git";
import type { AppState } from "./types";

/** `dropAgentEntries` destructures every per-agent side map, so all must exist. */
const EMPTY_MAPS = {
  managedLogs: {},
  transcriptLoading: {},
  transcriptLoaded: {},
  managedBusy: {},
  turnStartedAt: {},
  usage: {},
  gitStates: {},
  prStates: {},
  prChecks: {},
  prComments: {},
  gitShortstats: {},
  composerSeeds: {},
  composerDrafts: {},
  delegations: {},
  delegationNotices: {},
  autopilot: {},
  autopilotVerdicts: {},
  unseenResults: {},
  rightPanelTabs: {},
};

const NOW = 100_000;
const LATE = NOW + DELEGATION_GIVE_UP_GRACE_MS + 1;

function delegation(over: Partial<Delegation> = {}): Delegation {
  return {
    kind: "commit",
    prompt: "[app-action] commit",
    startedAt: NOW,
    sawRunning: false,
    sawGitOp: false,
    queued: false,
    ...over,
  };
}

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

const modified = {
  path: "a.ts",
  kind: "modified" as const,
  staged: false,
  additions: 1,
  deletions: 0,
};

function plan(
  delegations: Record<string, Delegation>,
  statuses: Record<string, AgentStatus>,
  over: {
    gitStates?: Record<string, GitState>;
    prStates?: Record<string, PrState | null>;
    now?: number;
  } = {},
): DelegationEffect[] {
  return planDelegationPass({
    delegations,
    statuses,
    gitStates: over.gitStates ?? {},
    prStates: over.prStates ?? {},
    prChecks: {},
    now: over.now ?? NOW,
  });
}

describe("checkoutKey", () => {
  it("round-trips through splitCheckoutKey, primary and secondary alike", () => {
    expect(splitCheckoutKey(checkoutKey("a1"))).toEqual({ agentId: "a1" });
    expect(splitCheckoutKey(checkoutKey("a1", "web"))).toEqual({ agentId: "a1", subdir: "web" });
    // Splitting on the FIRST separator keeps a subdir containing "::" intact.
    expect(splitCheckoutKey(checkoutKey("a1", "od::d"))).toEqual({
      agentId: "a1",
      subdir: "od::d",
    });
  });
});

describe("planDelegationPass advances delegations wherever they live", () => {
  it("resolves a delegation on an agent nobody is looking at (bug 1)", () => {
    // The pass has no notion of "selected" — an idle agent whose target is
    // reached and whose git op landed resolves, panel or no panel.
    const effects = plan(
      { a1: delegation({ sawGitOp: true, sawRunning: true }) },
      { a1: "idle" },
      { gitStates: { a1: git() } },
    );
    expect(effects).toEqual([
      {
        do: "resolve",
        key: "a1",
        agentId: "a1",
        subdir: undefined,
        notice: "Agent committed your changes",
      },
    ]);
  });

  it("dequeues a trigger held behind a running turn once it settles (bug 1, the broken half)", () => {
    // This is what never happened before: `updateAll` on a *running* agent left
    // the trigger undelivered because no watcher was mounted to dequeue it.
    expect(plan({ a1: delegation({ queued: true }) }, { a1: "running" })).toEqual([]);
    expect(plan({ a1: delegation({ queued: true }) }, { a1: "idle" })).toEqual([
      { do: "dequeue", key: "a1" },
    ]);
  });

  it("gives up on a settled agent that never reached the target, once armed", () => {
    const d = delegation({ sawRunning: true });
    expect(
      plan({ a1: d }, { a1: "idle" }, { gitStates: { a1: git({ files: [modified] }) } }),
    ).toEqual([
      { do: "give-up", key: "a1", notice: "Agent finished — review the chat for details" },
    ]);
  });

  it("calls a settled fix-checks its normal ending, not an abandonment", () => {
    // fix-checks never resolves from state (CI takes minutes), so the give-up
    // path IS its success path and must not read as a failure.
    expect(
      plan({ a1: delegation({ kind: "fix-checks", sawRunning: true }) }, { a1: "idle" }),
    ).toEqual([{ do: "give-up", key: "a1", notice: "Agent finished — checks are re-running" }]);
  });

  it("drops a delegation whose agent was archived or discarded under it", () => {
    expect(plan({ a1: delegation() }, {})).toEqual([{ do: "drop-orphan", key: "a1" }]);
  });

  it("waits out an unarmed delegation rather than giving up during the grace window", () => {
    expect(plan({ a1: delegation() }, { a1: "idle" })).toEqual([]);
    expect(plan({ a1: delegation() }, { a1: "idle" }, { now: LATE })).toEqual([
      { do: "give-up", key: "a1", notice: "Agent finished — review the chat for details" },
    ]);
  });
});

describe("planDelegationPass is per checkout, not per agent (bug 2)", () => {
  const key = { primary: checkoutKey("a1"), web: checkoutKey("a1", "web") };

  it("judges each checkout against its OWN state", () => {
    // Same agent, same kind, both idle with their op landed — but only the
    // primary's tree is clean, so only it resolves.
    const effects = plan(
      {
        [key.primary]: delegation({ sawGitOp: true, sawRunning: true }),
        [key.web]: delegation({ sawGitOp: true, sawRunning: true, subdir: "web" }),
      },
      { a1: "idle" },
      { gitStates: { [key.primary]: git(), [key.web]: git({ files: [modified] }) } },
    );
    expect(effects).toEqual([
      {
        do: "resolve",
        key: key.primary,
        agentId: "a1",
        subdir: undefined,
        notice: "Agent committed your changes",
      },
      { do: "give-up", key: key.web, notice: "Agent finished — review the chat for details" },
    ]);
  });

  it("dequeues at most one delegation per agent per pass", () => {
    // Both are queued behind the same turn. Delivering both would coalesce the
    // second into the first's turn — the exact thing `queued` prevents.
    const effects = plan(
      {
        [key.primary]: delegation({ queued: true }),
        [key.web]: delegation({ queued: true, subdir: "web" }),
      },
      { a1: "idle" },
    );
    expect(effects).toEqual([{ do: "dequeue", key: key.primary }]);
  });

  it("still dequeues concurrently for different agents", () => {
    const effects = plan(
      { a1: delegation({ queued: true }), a2: delegation({ queued: true }) },
      { a1: "idle", a2: "idle" },
    );
    expect(effects.map((e) => e.key).sort()).toEqual(["a1", "a2"]);
  });
});

// ── store wiring ──────────────────────────────────────────────────────────────

const makeStore = (statusById: Record<string, AgentStatus>) => {
  const sendUserMessage = vi.fn();
  const store = create<AppState>()((...a) => ({ ...createGitSlice(...a) }) as AppState);
  store.setState({
    sendUserMessage,
    workspace: {
      agents: Object.entries(statusById).map(([id, status]) => ({ id, status })),
      // biome-ignore lint/suspicious/noExplicitAny: minimal workspace fixture
    } as any,
    // biome-ignore lint/suspicious/noExplicitAny: partial store seed
  } as any);
  return { store, sendUserMessage };
};

describe("delegateAction keys by checkout", () => {
  it("lets one multi-repo agent hold a delegation per checkout (bug 2)", () => {
    const { store, sendUserMessage } = makeStore({ a1: "idle" });

    store.getState().delegateAction("a1", "commit", "[app-action] commit");
    store.getState().delegateAction("a1", "resolve", "[app-action] resolve-conflicts", "web");

    expect(Object.keys(store.getState().delegations).sort()).toEqual(["a1", "a1::web"]);
    expect(store.getState().delegations["a1::web"].kind).toBe("resolve");
    // Both were dispatched to an idle agent, so both triggers went out.
    expect(sendUserMessage).toHaveBeenCalledTimes(2);
  });

  it("holds the trigger instead of sending it when the agent is mid-turn", () => {
    const { store, sendUserMessage } = makeStore({ a1: "running" });

    store.getState().delegateAction("a1", "commit", "[app-action] commit");

    expect(store.getState().delegations.a1.queued).toBe(true);
    expect(sendUserMessage).not.toHaveBeenCalled();
  });
});

describe("markDelegationActed attributes an agent-scoped ack", () => {
  it("acks every checkout of that agent whose kind the op belongs to", () => {
    // The backend event reports the op, not the checkout it ran in, so the ack
    // is agent-wide. Safe because resolution ANDs it with each checkout's own
    // target snapshot — see the store's interface doc.
    const { store } = makeStore({ a1: "idle", a2: "idle" });
    store.setState({
      delegations: {
        a1: delegation(),
        "a1::web": delegation({ subdir: "web" }),
        a2: delegation(),
      },
    });

    store.getState().markDelegationActed("a1", "git_commit");

    expect(store.getState().delegations.a1.sawGitOp).toBe(true);
    expect(store.getState().delegations["a1::web"].sawGitOp).toBe(true);
    // A different agent's delegation is untouched.
    expect(store.getState().delegations.a2.sawGitOp).toBe(false);
  });

  it("ignores an op from another kind's playbook, and any op while queued", () => {
    const { store } = makeStore({ a1: "idle" });
    store.setState({
      delegations: {
        a1: delegation({ kind: "commit" }),
        "a1::web": delegation({ kind: "commit", queued: true, subdir: "web" }),
      },
    });

    // `git_push` is not in `commit`'s playbook.
    store.getState().markDelegationActed("a1", "git_push");
    expect(store.getState().delegations.a1.sawGitOp).toBe(false);

    // Right kind, but the queued one's trigger hasn't been delivered yet — the
    // op belongs to the turn it is waiting behind.
    store.getState().markDelegationActed("a1", "git_commit");
    expect(store.getState().delegations.a1.sawGitOp).toBe(true);
    expect(store.getState().delegations["a1::web"].sawGitOp).toBe(false);
  });
});

describe("markDelegationDequeued delivers exactly one held trigger", () => {
  it("sends that checkout's prompt to its agent and clears `queued`", () => {
    const { store, sendUserMessage } = makeStore({ a1: "idle" });
    store.setState({
      delegations: {
        a1: delegation({ queued: true, prompt: "primary" }),
        "a1::web": delegation({ queued: true, prompt: "web", subdir: "web" }),
      },
    });

    store.getState().markDelegationDequeued("a1::web");

    expect(sendUserMessage).toHaveBeenCalledExactlyOnceWith("a1", "web");
    expect(store.getState().delegations["a1::web"].queued).toBe(false);
    // The sibling is left alone — one dequeue, one delivery.
    expect(store.getState().delegations.a1.queued).toBe(true);
  });

  it("is idempotent, so a repeated pass can't double-deliver", () => {
    const { store, sendUserMessage } = makeStore({ a1: "idle" });
    store.setState({ delegations: { a1: delegation({ queued: true, prompt: "go" }) } });

    store.getState().markDelegationDequeued("a1");
    store.getState().markDelegationDequeued("a1");

    expect(sendUserMessage).toHaveBeenCalledTimes(1);
  });
});

describe("clearDelegation", () => {
  it("drops only the named checkout", () => {
    const { store } = makeStore({ a1: "idle" });
    store.setState({
      delegations: { a1: delegation(), "a1::web": delegation({ subdir: "web" }) },
    });

    store.getState().clearDelegation("a1");

    expect(Object.keys(store.getState().delegations)).toEqual(["a1::web"]);
  });
});

describe("dropAgentEntries", () => {
  it("takes every checkout's delegation and notice with the agent", () => {
    // Per-checkout keying means an agent owns `id` AND `id::subdir` entries; a
    // by-agent-key delete would leave the secondaries behind as orphans that
    // `useTrackedCheckoutKeys` would go on polling.
    const patch = dropAgentEntries(
      {
        ...EMPTY_MAPS,
        delegations: {
          a1: delegation(),
          "a1::web": delegation({ subdir: "web" }),
          a2: delegation(),
        },
        delegationNotices: { a1: "done", "a1::web": "done", a2: "done" },
        // biome-ignore lint/suspicious/noExplicitAny: partial state fixture
      } as any,
      "a1",
    );

    expect(Object.keys(patch.delegations ?? {})).toEqual(["a2"]);
    expect(Object.keys(patch.delegationNotices ?? {})).toEqual(["a2"]);
  });
});
