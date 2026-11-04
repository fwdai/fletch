// The publish-approval queue: what reaches the prompt, what takes it down, and
// how a resync replaces it. What the user already authorized (a delegation's or
// an autopilot-enrolled checkout's own pushes) the host approves without
// asking, so it never arrives here — that policy is the host's, tested there.

import { describe, expect, it, vi } from "vitest";
import { create } from "zustand";
import type {
  BoxEgressEvent,
  BoxStateEvent,
  BoxVm,
  EgressAnswer,
  EgressPrompt,
  EgressPromptResolved,
  PublishApproval,
  PublishApprovalResolved,
} from "@/api";
import { type EnvironmentEntry, LOCAL_ENVIRONMENT_ID, setEnvironmentsSource } from "./environments";
import { createSandboxSlice } from "./sandbox";

const { answerPublishApproval, listPublishApprovals, boxList, boxEgressAnswer, boxEgressPending } =
  vi.hoisted(() => ({
    answerPublishApproval: vi.fn(),
    listPublishApprovals: vi.fn(),
    boxList: vi.fn(),
    boxEgressAnswer: vi.fn(),
    boxEgressPending: vi.fn(),
  }));
vi.mock("@/api", () => ({
  api: { answerPublishApproval, listPublishApprovals, boxList, boxEgressAnswer, boxEgressPending },
}));

/** The slice creator and the actions, loosely typed: the test store carries
 *  only the sandbox slice, not the whole AppState. */
type SliceFn = (set: unknown, get: unknown) => Record<string, unknown>;
type Recv = (r: PublishApproval) => void;
type Resolve = (e: PublishApprovalResolved) => void;
type Load = () => Promise<void>;

describe("receivePublishApproval", () => {
  function store() {
    answerPublishApproval.mockClear();
    return create<Record<string, unknown>>((set, get) => ({
      ...(createSandboxSlice as unknown as SliceFn)(set, get),
    }));
  }

  const request = (over: Partial<PublishApproval> = {}): PublishApproval => ({
    id: "r1",
    agent_id: "a1",
    op: "git_push",
    detail: "push fix/ci",
    ...over,
  });

  it("queues each prompt, oldest first, and answers nothing by itself", () => {
    const s = store();
    (s.getState().receivePublishApproval as Recv)(request());
    (s.getState().receivePublishApproval as Recv)(request({ id: "r2", op: "open_pr" }));
    expect(answerPublishApproval).not.toHaveBeenCalled();
    expect((s.getState().pendingPublishApprovals as PublishApproval[]).map((r) => r.id)).toEqual([
      "r1",
      "r2",
    ]);
  });

  // `publish:approval-resolved`: the question ended somewhere else, so the
  // prompt has to come down without another word to the engine — answering a
  // request it has already dropped would reach nothing.
  it("drops a queued prompt the engine says is over, and answers nothing", () => {
    const s = store();
    (s.getState().receivePublishApproval as Recv)(request());
    (s.getState().receivePublishApproval as Recv)(request({ id: "r2" }));

    (s.getState().resolvePublishApproval as Resolve)({ id: "r1", outcome: "expired" });

    const left = s.getState().pendingPublishApprovals as PublishApproval[];
    expect(left.map((r) => r.id)).toEqual(["r2"]);
    expect(answerPublishApproval).not.toHaveBeenCalled();
  });

  it("ignores a resolution for a prompt it never queued", () => {
    const s = store();
    (s.getState().resolvePublishApproval as Resolve)({ id: "never-seen", outcome: "approved" });
    expect(s.getState().pendingPublishApprovals).toEqual([]);
  });

  // `publish:approval-requested` reaches only the clients connected when it
  // fired, so a window that has just switched to a host — or reconnected to one
  // — has to ask what is waiting rather than wait for an event that has been
  // and gone.
  describe("loadPendingPublishApprovals", () => {
    const remote = (ops: string[]): EnvironmentEntry => ({
      id: "host-1",
      name: "Cloud box",
      kind: "remote",
      connection: "connected",
      protocol: { version: 2, ops, events: [], features: [] },
    });

    const activeIs = (entry?: EnvironmentEntry) =>
      setEnvironmentsSource(() => ({
        activeEnvironmentId: entry?.id ?? LOCAL_ENVIRONMENT_ID,
        environments: entry ? { [entry.id]: entry } : {},
      }));

    it("replaces the queue with what the host is still blocked on", async () => {
      listPublishApprovals.mockResolvedValue([request({ id: "raised-while-away" })]);
      activeIs(remote(["approvals_list"]));
      const s = store();
      // A card this window holds that the host has since resolved: wholesale,
      // so it goes.
      (s.getState().receivePublishApproval as Recv)(request({ id: "answered-elsewhere" }));

      await (s.getState().loadPendingPublishApprovals as Load)();

      expect((s.getState().pendingPublishApprovals as PublishApproval[]).map((r) => r.id)).toEqual([
        "raised-while-away",
      ]);
    });

    it("keeps a prompt that was raised while the list was in flight", async () => {
      // The host forwards events and writes op responses from separate tasks,
      // so a publish gated a moment after the queue was read reaches us first.
      // Replacing the queue wholesale used to drop it — the host then waits out
      // its timeout with nothing on screen to answer it.
      activeIs(remote(["approvals_list"]));
      const s = store();
      listPublishApprovals.mockImplementation(async () => {
        (s.getState().receivePublishApproval as Recv)(request({ id: "raised-mid-flight" }));
        return [request({ id: "already-waiting" })];
      });

      await (s.getState().loadPendingPublishApprovals as Load)();

      expect((s.getState().pendingPublishApprovals as PublishApproval[]).map((r) => r.id)).toEqual([
        "already-waiting",
        "raised-mid-flight",
      ]);
    });

    it("does not put back a prompt the host resolved while the list was in flight", async () => {
      activeIs(remote(["approvals_list"]));
      const s = store();
      listPublishApprovals.mockImplementation(async () => {
        (s.getState().resolvePublishApproval as Resolve)({ id: "over", outcome: "denied" });
        return [request({ id: "over" }), request({ id: "still-waiting" })];
      });

      await (s.getState().loadPendingPublishApprovals as Load)();

      expect((s.getState().pendingPublishApprovals as PublishApproval[]).map((r) => r.id)).toEqual([
        "still-waiting",
      ]);
    });

    it("lets the newest of two overlapping loads own the queue", async () => {
      // Every handshake starts a load without waiting for the last, so two are
      // out at once and can answer out of order. The older one answering last
      // used to land its stale snapshot — and, since the buffer is the newest
      // claim's, it would have replayed almost nothing over it.
      activeIs(remote(["approvals_list"]));
      const s = store();
      const answers: ((rows: PublishApproval[]) => void)[] = [];
      listPublishApprovals.mockImplementation(
        () => new Promise<PublishApproval[]>((resolve) => answers.push(resolve)),
      );

      const older = (s.getState().loadPendingPublishApprovals as Load)();
      const newer = (s.getState().loadPendingPublishApprovals as Load)();
      // Raised while both are out: it belongs to the newer read, the only one
      // whose snapshot will be written.
      (s.getState().receivePublishApproval as Recv)(request({ id: "raised-mid-flight" }));

      answers[1]([request({ id: "from-the-newer-read" })]);
      await newer;
      answers[0]([request({ id: "from-the-older-read" })]);
      await older;

      expect((s.getState().pendingPublishApprovals as PublishApproval[]).map((r) => r.id)).toEqual([
        "from-the-newer-read",
        "raised-mid-flight",
      ]);
    });

    it("asks neither this Mac nor a host too old for the op", async () => {
      listPublishApprovals.mockClear();
      listPublishApprovals.mockResolvedValue([]);
      const s = store();

      activeIs();
      await (s.getState().loadPendingPublishApprovals as Load)();
      activeIs(remote([]));
      await (s.getState().loadPendingPublishApprovals as Load)();

      expect(listPublishApprovals).not.toHaveBeenCalled();
    });
  });
});

describe("refreshBoxVms", () => {
  const state = (id: string, phase: BoxStateEvent["phase"]): BoxStateEvent => ({
    agent_id: id,
    phase,
  });
  const vm = (id: string, phase: BoxStateEvent["phase"] = "stopped"): BoxVm => ({
    agent_id: id,
    name: id,
    project_id: "p",
    created_at: "1",
    state: state(id, phase),
  });
  const ids = (s: { getState: () => Record<string, unknown> }) =>
    (s.getState().boxVms as BoxVm[] | null)?.map((v) => v.agent_id);
  const activeIs = (id: string) =>
    setEnvironmentsSource(() => ({
      activeEnvironmentId: id,
      environments: {
        [id]: { id, name: id, kind: "remote", connection: "connected" } as EnvironmentEntry,
      },
    }));

  function store() {
    boxList.mockReset();
    activeIs(LOCAL_ENVIRONMENT_ID);
    return create<Record<string, unknown>>((set, get) => ({
      ...(createSandboxSlice as unknown as SliceFn)(set, get),
    }));
  }
  const refresh = (s: ReturnType<typeof store>) => (s.getState().refreshBoxVms as Load)();

  it("holds the host's list and each VM's phase", async () => {
    const s = store();
    boxList.mockResolvedValue([vm("a", "running"), vm("b")]);

    await refresh(s);

    expect(ids(s)).toEqual(["a", "b"]);
    expect(s.getState().boxStates).toEqual({ a: state("a", "running"), b: state("b", "stopped") });
  });

  it("drops a VM the host no longer lists, and its phase", async () => {
    const s = store();
    boxList.mockResolvedValue([vm("hidden-chat"), vm("kept")]);
    await refresh(s);

    boxList.mockResolvedValue([vm("kept")]);
    await refresh(s);

    expect(ids(s)).toEqual(["kept"]);
    expect(s.getState().boxStates).toEqual({ kept: state("kept", "stopped") });
  });

  it("drops an answer that lands after a switch to another host", async () => {
    const s = store();
    boxList.mockImplementation(async () => {
      activeIs("other-host");
      return [vm("fuji", "running")];
    });

    await refresh(s);

    expect(s.getState().boxVms).toBeNull();
    expect(s.getState().boxStates).toEqual({});
  });

  it("lets the newest of two overlapping reads own the list", async () => {
    const s = store();
    let finishOlder: (vms: BoxVm[]) => void = () => {};
    boxList.mockReturnValueOnce(new Promise((resolve) => (finishOlder = resolve)));
    const older = refresh(s);
    boxList.mockResolvedValueOnce([vm("newer")]);
    await refresh(s);

    // The older read still has the VM a discard just removed.
    finishOlder([vm("newer"), vm("discarded")]);
    await older;

    expect(ids(s)).toEqual(["newer"]);
  });

  it("keeps a phase box:state reported while the read was in flight", async () => {
    const s = store();
    boxList.mockImplementation(async () => {
      (s.getState().applyBoxState as (e: BoxStateEvent) => void)(state("a", "stopping"));
      return [vm("a", "running")];
    });

    await refresh(s);

    expect((s.getState().boxStates as Record<string, BoxStateEvent>).a.phase).toBe("stopping");
  });

  it("stays unknown on a host that cannot answer", async () => {
    const s = store();
    boxList.mockRejectedValue(new Error("unknown op"));

    await refresh(s);

    expect(s.getState().boxVms).toBeNull();
  });
});

// The held-host queue of the Ask me posture: one entry per prompt, answered
// through the host and dropped at once, taken down by `box:egress-resolved`,
// and the refusal that follows an unanswered one raises no second toast.
describe("egress prompts", () => {
  function store() {
    boxEgressAnswer.mockClear();
    boxEgressAnswer.mockResolvedValue(undefined);
    return create<Record<string, unknown>>((set, get) => ({
      ...(createSandboxSlice as unknown as SliceFn)(set, get),
    }));
  }
  const prompt = (over: Partial<EgressPrompt> = {}): EgressPrompt => ({
    id: "h1",
    agent_id: "a1",
    project_id: "p1",
    host: "api.stripe.com",
    port: 443,
    requested_at: "2026-11-04T00:00:00Z",
    ...over,
  });
  const pending = (s: ReturnType<typeof store>) =>
    (s.getState().pendingEgressPrompts as EgressPrompt[]).map((p) => p.id);
  const receive = (s: ReturnType<typeof store>, p: EgressPrompt) =>
    (s.getState().receiveEgressPrompt as (p: EgressPrompt) => void)(p);

  it("queues each held host once and answers nothing by itself", () => {
    const s = store();
    receive(s, prompt());
    receive(s, prompt());
    receive(s, prompt({ id: "h2", host: "docs.python.org" }));
    expect(pending(s)).toEqual(["h1", "h2"]);
    expect(boxEgressAnswer).not.toHaveBeenCalled();
  });

  it("drops the prompt as it answers, and when the host says it is over", async () => {
    const s = store();
    receive(s, prompt());
    receive(s, prompt({ id: "h2", host: "docs.python.org" }));
    await (s.getState().answerEgressPrompt as (id: string, a: EgressAnswer) => Promise<void>)(
      "h1",
      "project",
    );
    expect(boxEgressAnswer).toHaveBeenCalledWith("h1", "project");
    expect(pending(s)).toEqual(["h2"]);
    (s.getState().resolveEgressPrompt as (e: EgressPromptResolved) => void)({
      id: "h2",
      outcome: "expired",
    });
    expect(pending(s)).toEqual([]);
  });

  it("does not raise the couldn't-reach toast for a host the user was asked about", () => {
    const s = store();
    receive(s, prompt());
    (s.getState().resolveEgressPrompt as (e: EgressPromptResolved) => void)({
      id: "h1",
      outcome: "expired",
    });
    const refused = (e: BoxEgressEvent) =>
      (s.getState().receiveBoxEgress as (e: BoxEgressEvent) => void)(e);
    refused({ agent_id: "a1", host: "api.stripe.com", port: 443, allowed: false, quiet: false });
    refused({ agent_id: "a1", host: "other.test", port: 443, allowed: false, quiet: false });
    const blocked = s.getState().boxEgressBlocked as { host: string; seen: boolean }[];
    expect(blocked.find((b) => b.host === "api.stripe.com")?.seen).toBe(true);
    expect(blocked.find((b) => b.host === "other.test")?.seen).toBe(false);
  });
});
