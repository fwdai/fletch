// The publish pre-authorization policy is pure, so it is covered here as a
// function of its inputs.
//
// The load-bearing case is the *unattended* one. If `publishPreAuthorized` fails
// to recognise an autopilot-driven push, the backend prompt goes unanswered, its
// 120s timeout denies it, the rung fails, autopilot retries it to its budget and
// then marks the checkout stuck — a run nobody was watching, stopped. So the
// autopilot ground is asserted from both directions, and its independence from the
// delegation ground is asserted too.

import { describe, expect, it, vi } from "vitest";
import { create } from "zustand";
import type { PublishApproval } from "@/api";
import type { AutopilotState } from "@/autopilot";
import { newEnrollment } from "@/autopilot";
import type { Delegation, DelegationKind } from "@/delegation";
import { checkoutKey } from "./git";
import { type PublishAuthorityState, publishPreAuthorized } from "./publishApproval";
import { createSandboxSlice } from "./sandbox";

const { answerPublishApproval } = vi.hoisted(() => ({ answerPublishApproval: vi.fn() }));
vi.mock("@/api", () => ({ api: { answerPublishApproval } }));

/** The slice creator and the action, loosely typed: the test store carries only
 *  the sandbox slice plus the two maps the policy reads, not the whole AppState. */
type SliceFn = (set: unknown, get: unknown) => Record<string, unknown>;
type Recv = (r: PublishApproval) => void;

const KEY = checkoutKey("a1");
const SECOND_REPO = checkoutKey("a1", "web");

function state(over: Partial<PublishAuthorityState> = {}): PublishAuthorityState {
  return { autopilot: {}, delegations: {}, ...over };
}

function enrolled(over: Partial<AutopilotState> = {}): AutopilotState {
  return { ...newEnrollment(), ...over };
}

function delegation(kind: DelegationKind): Delegation {
  return { kind } as Delegation;
}

describe("autopilot's standing authorization", () => {
  it("covers a push on the checkout it is driving", () => {
    const s = state({ autopilot: { [KEY]: enrolled() } });
    expect(publishPreAuthorized("git_push", KEY, s)).toBe(true);
  });

  it("does not cover opening a pull request", () => {
    // Every autopilot rung works on a PR that already exists, so needing this
    // would mean the rung set changed — and creating a public artifact under the
    // user's identity should not ride on an enrollment made for CI fixes.
    const s = state({ autopilot: { [KEY]: enrolled() } });
    expect(publishPreAuthorized("open_pr", KEY, s)).toBe(false);
  });

  it("does not cover a paused enrollment", () => {
    // Paused autopilot dispatches nothing, so a publish arriving then did not
    // come from it and is not covered by the consent to run it.
    const s = state({ autopilot: { [KEY]: enrolled({ paused: true }) } });
    expect(publishPreAuthorized("git_push", KEY, s)).toBe(false);
  });

  it("does not leak across checkouts of the same agent", () => {
    const s = state({ autopilot: { [KEY]: enrolled() } });
    expect(publishPreAuthorized("git_push", SECOND_REPO, s)).toBe(false);
  });

  it("holds with no delegation in flight", () => {
    // The two grounds are independent: autopilot pushes without a user click, so
    // requiring both would stall exactly the case this exists for.
    const s = state({ autopilot: { [KEY]: enrolled() }, delegations: {} });
    expect(publishPreAuthorized("git_push", KEY, s)).toBe(true);
  });
});

describe("a live delegation's authorization", () => {
  it("covers the ops its own playbook publishes", () => {
    for (const [kind, op] of [
      ["push", "git_push"],
      ["commit-push", "git_push"],
      ["fix-checks", "git_push"],
      ["open-pr", "open_pr"],
      ["commit-pr", "open_pr"],
    ] as const) {
      const s = state({ delegations: { [KEY]: delegation(kind) } });
      expect(publishPreAuthorized(op, KEY, s), `${kind} → ${op}`).toBe(true);
    }
  });

  it("cannot launder an op its playbook never performs", () => {
    // The guard that matters: a delegation the user started for one thing must
    // not authorize a different publish the agent chose on its own.
    const s = state({ delegations: { [KEY]: delegation("commit") } });
    expect(publishPreAuthorized("git_push", KEY, s)).toBe(false);
    expect(publishPreAuthorized("open_pr", KEY, s)).toBe(false);
    const pushOnly = state({ delegations: { [KEY]: delegation("push") } });
    expect(publishPreAuthorized("open_pr", KEY, pushOnly)).toBe(false);
  });

  it("is scoped to its own checkout", () => {
    const s = state({ delegations: { [KEY]: delegation("push") } });
    expect(publishPreAuthorized("git_push", SECOND_REPO, s)).toBe(false);
  });
});

describe("with no authority at all", () => {
  it("authorizes nothing", () => {
    // An agent that decided to publish by itself: exactly what the prompt is for.
    for (const op of ["git_push", "open_pr"]) {
      expect(publishPreAuthorized(op, KEY, state())).toBe(false);
    }
  });
});

// ── the wiring, end to end ──────────────────────────────────────────────────
//
// The policy being right is necessary but not sufficient: the slice has to consult
// it and answer the backend. These drive the REAL `receivePublishApproval` (an
// earlier version of this block re-implemented its body, which would have passed
// against broken code) and assert what the backend observes — an answer, or a
// queued prompt. That is what decides whether an unattended run proceeds.

describe("receivePublishApproval", () => {
  function store(over: Partial<PublishAuthorityState> = {}) {
    answerPublishApproval.mockClear();
    return create<Record<string, unknown>>((set, get) => ({
      ...(createSandboxSlice as unknown as SliceFn)(set, get),
      ...state(over),
    }));
  }

  const request = (over: Partial<PublishApproval> = {}): PublishApproval => ({
    id: "r1",
    agent_id: "a1",
    op: "git_push",
    detail: "push fix/ci",
    ...over,
  });

  it("answers an autopilot push without ever queueing a prompt", async () => {
    const s = store({ autopilot: { [KEY]: enrolled() } });
    (s.getState().receivePublishApproval as Recv)(request());
    await Promise.resolve();
    expect(answerPublishApproval).toHaveBeenCalledWith("r1", true);
    expect(
      s.getState().pendingPublishApprovals,
      "an unattended run must never wait on a prompt",
    ).toEqual([]);
  });

  it("queues a prompt for a publish the agent chose by itself", () => {
    const s = store();
    (s.getState().receivePublishApproval as Recv)(request());
    expect(answerPublishApproval).not.toHaveBeenCalled();
    expect(s.getState().pendingPublishApprovals).toHaveLength(1);
  });

  it("queues a pull request even while autopilot is driving", () => {
    const s = store({ autopilot: { [KEY]: enrolled() } });
    (s.getState().receivePublishApproval as Recv)(request({ op: "open_pr" }));
    expect(answerPublishApproval).not.toHaveBeenCalled();
    expect(s.getState().pendingPublishApprovals).toHaveLength(1);
  });
});
