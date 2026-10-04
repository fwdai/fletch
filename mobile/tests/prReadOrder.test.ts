// The PR reads against the host watcher's events, over the mock host: a reply
// to a read issued before an event must not land on top of it, since the
// watcher sends each change once. The order itself is
// @desktop/store/prWriteOrder, tested there.

import type { AgentPrStatus, PrChecks, PrComments, PrState } from "@desktop/api/types/pr";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { prChecks as fixtureChecks, prStates as fixtureStates } from "../src/remote/mock/fixtures";
import { api, client, useStore } from "../src/store";

const state = () => useStore.getState();

const hostEvent = (event: string, payload: unknown) =>
  (
    client as unknown as { dispatchEvent(frame: { event: string; payload: unknown }): void }
  ).dispatchEvent({ event, payload });

/** A host answer held back until the test lets it land. */
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((r) => {
    resolve = r;
  });
  return { promise, resolve };
}

const AGENT = "arabia";
const OTHER = "pamukkale";
const open: PrState = { ...(fixtureStates.kamakura as PrState), number: 900 };
const merged: PrState = { ...open, state: "merged" };
const green: PrChecks = fixtureChecks.kamakura as PrChecks;
const red: PrChecks = { ...green, rollup: "failing", passed: 13, failed: 1 };
const threads = (body: string): PrComments => ({
  unresolved: [
    {
      id: `t-${body}`,
      author: "greptile",
      is_bot: true,
      body,
      path: "src/main.tsx",
      line: 1,
      url: "https://example.test",
      replies: 0,
      we_replied_last: false,
    },
  ],
});

const forget = <T>(map: Record<string, T>) => {
  const { [AGENT]: _a, [OTHER]: _o, ...rest } = map;
  return rest;
};

beforeAll(async () => {
  await state().init();
  await vi.waitFor(() => expect(state().connection).toBe("connected"), { timeout: 5000 });
  const protocol = state().protocol;
  // The mock host predates the threads read; the race is the same either way.
  useStore.setState({
    protocol: protocol && { ...protocol, ops: [...protocol.ops, "get_pr_threads"] },
  });
});

beforeEach(() => {
  useStore.setState((s) => ({
    prStates: forget(s.prStates),
    prChecks: forget(s.prChecks),
    prComments: forget(s.prComments),
  }));
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("a PR read against the watcher's events", () => {
  it("keeps the threads an event delivered while loadPrThreads was out", async () => {
    const reply = deferred<PrComments>();
    vi.spyOn(api, "getPrThreads").mockReturnValueOnce(reply.promise);
    const load = state().loadPrThreads(AGENT);

    hostEvent("pr:threads_changed", {
      agent_id: AGENT,
      subdir: null,
      comments: threads("newer"),
      new_thread_ids: ["t-newer"],
    });
    reply.resolve(threads("older"));
    await load;

    expect(state().prComments[AGENT]).toEqual(threads("newer"));
  });

  it("keeps the checks an event delivered while loadPrLive was out, taking its state", async () => {
    const reply = deferred<{ state: PrState; checks: PrChecks | null }>();
    vi.spyOn(api, "getPrLive").mockReturnValueOnce(reply.promise);
    const load = state().loadPrLive(AGENT);

    hostEvent("pr:checks_changed", { agent_id: AGENT, subdir: null, number: 900, checks: red });
    reply.resolve({ state: { ...open, title: "renamed" }, checks: green });
    await load;

    expect(state().prChecks[AGENT]).toEqual(red);
    // The event said nothing about the state, so the read's is the newest.
    expect(state().prStates[AGENT]?.title).toBe("renamed");
  });

  it("keeps the state an event delivered while loadPrLive was out", async () => {
    // The event's own follow-up `loadGit` reads what the host now says.
    vi.spyOn(api, "getPrState").mockResolvedValue(merged);
    vi.spyOn(api, "getPrChecks").mockResolvedValue(null);
    const reply = deferred<{ state: PrState; checks: PrChecks | null }>();
    vi.spyOn(api, "getPrLive").mockReturnValueOnce(reply.promise);
    const load = state().loadPrLive(AGENT);

    hostEvent("pr:state_changed", { agent_id: AGENT, subdir: null, state: merged });
    reply.resolve({ state: open, checks: green });
    await load;

    expect(state().prStates[AGENT]?.state).toBe("merged");
  });

  it("skips, row by row, the agents an event stamped while loadPrStatus was out", async () => {
    const reply = deferred<Record<string, AgentPrStatus>>();
    vi.spyOn(api, "getAllPrStatus").mockReturnValueOnce(reply.promise);
    const load = state().loadPrStatus();

    hostEvent("pr:checks_changed", { agent_id: AGENT, subdir: null, number: 900, checks: red });
    // A PR gone, so no follow-up `loadGit` either.
    hostEvent("pr:state_changed", { agent_id: OTHER, subdir: null, state: null });
    reply.resolve({
      [AGENT]: { state: open, checks: green },
      [OTHER]: { state: open, checks: green },
    });
    await load;

    // Each agent keeps only what an event outran: its slice, not the row.
    expect(state().prChecks[AGENT]).toEqual(red);
    expect(state().prStates[AGENT]).toEqual(open);
    expect(state().prStates[OTHER]).toBeNull();
    expect(state().prChecks[OTHER]).toEqual(green);
  });

  it("applies a read issued after the event — the event only outranks older reads", async () => {
    hostEvent("pr:threads_changed", {
      agent_id: AGENT,
      subdir: null,
      comments: threads("event"),
      new_thread_ids: ["t-event"],
    });
    hostEvent("pr:checks_changed", { agent_id: AGENT, subdir: null, number: 900, checks: red });
    vi.spyOn(api, "getPrThreads").mockResolvedValueOnce(threads("read"));
    vi.spyOn(api, "getPrLive").mockResolvedValueOnce({ state: open, checks: green });
    vi.spyOn(api, "getAllPrStatus").mockResolvedValueOnce({
      [OTHER]: { state: merged, checks: null },
    });

    await state().loadPrThreads(AGENT);
    await state().loadPrLive(AGENT);
    await state().loadPrStatus();

    expect(state().prComments[AGENT]).toEqual(threads("read"));
    expect(state().prChecks[AGENT]).toEqual(green);
    expect(state().prStates[OTHER]).toEqual(merged);
  });

  it("drops a reply from before a handshake, and takes the new host's first read", async () => {
    // The agent ids recur across hosts, so the old one's answer would land on
    // the new one's rows.
    hostEvent("pr:checks_changed", { agent_id: AGENT, subdir: null, number: 900, checks: red });
    const stale = deferred<Record<string, AgentPrStatus>>();
    const read = vi
      .spyOn(api, "getAllPrStatus")
      .mockReturnValueOnce(stale.promise)
      .mockResolvedValueOnce({ [AGENT]: { state: open, checks: green } });
    const load = state().loadPrStatus();

    await state().reconnect();
    // The handshake's own seed, issued after the old host's stamp: it applies.
    await vi.waitFor(() => expect(state().prChecks[AGENT]).toEqual(green));
    expect(read).toHaveBeenCalledTimes(2);

    stale.resolve({
      [AGENT]: { state: merged, checks: red },
      [OTHER]: { state: merged, checks: red },
    });
    await load;

    expect(state().prStates[AGENT]).toEqual(open);
    expect(state().prChecks[AGENT]).toEqual(green);
    expect(state().prStates[OTHER]).toBeUndefined();
  });
});
