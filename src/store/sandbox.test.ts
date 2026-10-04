// The publish-approval queue: what reaches the prompt, what takes it down, and
// how a resync replaces it. What the user already authorized (a delegation's or
// an autopilot-enrolled checkout's own pushes) the host approves without
// asking, so it never arrives here — that policy is the host's, tested there.

import { describe, expect, it, vi } from "vitest";
import { create } from "zustand";
import type { PublishApproval, PublishApprovalResolved } from "@/api";
import { type EnvironmentEntry, LOCAL_ENVIRONMENT_ID, setEnvironmentsSource } from "./environments";
import { createSandboxSlice } from "./sandbox";

const { answerPublishApproval, listPublishApprovals } = vi.hoisted(() => ({
  answerPublishApproval: vi.fn(),
  listPublishApprovals: vi.fn(),
}));
vi.mock("@/api", () => ({ api: { answerPublishApproval, listPublishApprovals } }));

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
