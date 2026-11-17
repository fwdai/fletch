// One store per host (store/host): built over that host's link, holding that
// host's data and the order of its reads and writes. Agent ids recur across
// hosts, so two stores must never share either. Nothing here dials: every
// answer is a spy on the link's api, so each store's own reads can be held and
// released in whatever order the case needs.

import type { AgentRecord, Workspace } from "@desktop/api/types/agent";
import type { GitState } from "@desktop/api/types/git";
import type { PrState } from "@desktop/api/types/pr";
import type { PublishApproval } from "@desktop/api/types/sandbox";
import { afterEach, describe, expect, it, vi } from "vitest";
import { type HostLink, linkOf, pairingLink } from "../src/host";
import { DEFAULT_PORT, type HostProtocol } from "../src/remote";
import { MOCK_HOST_KEY } from "../src/remote/mock";
import { type HostStore, hostStoreOf, storeOfLink } from "../src/store";

/** The same agent id on both hosts — the case that must not cross. */
const AGENT = "arabia";

const PROTOCOL: HostProtocol = {
  version: 3,
  ops: ["approvals_list", "read_session_page", "get_all_pr_status"],
  events: [],
  features: [],
};

/** Two hosts' stores, each answering for its own host. */
function twoHosts(): [HostStore, HostLink, HostStore, HostLink] {
  const a = linkOf("stores-host-a");
  const b = linkOf("stores-host-b");
  const storeA = hostStoreOf("stores-host-a");
  const storeB = hostStoreOf("stores-host-b");
  storeA.setState({ protocol: PROTOCOL });
  storeB.setState({ protocol: PROTOCOL });
  return [storeA, a, storeB, b];
}

/** A promise released by hand. */
function held<T>() {
  let release: (value: T) => void = () => {};
  const promise = new Promise<T>((resolve) => {
    release = resolve;
  });
  return { promise, release };
}

const git = (branch: string) => ({ branch, files: [] }) as unknown as GitState;
const pr = (title: string) => ({ number: 1, title, state: "open" }) as unknown as PrState;
const workspace = (name: string): Workspace =>
  ({
    projects: [],
    agents: [{ id: AGENT, name, status: "idle", repos: [] } as unknown as AgentRecord],
  }) as unknown as Workspace;

afterEach(() => vi.restoreAllMocks());

describe("a store per host", () => {
  it("hands back the same store for a host, and another host a store of its own", () => {
    const store = hostStoreOf("stores-host-a");
    expect(hostStoreOf("stores-host-a")).toBe(store);
    expect(storeOfLink(linkOf("stores-host-a"))).toBe(store);
    expect(hostStoreOf("stores-host-b")).not.toBe(store);
  });

  it("makes a pairing's store its host's once the host answers, as the link does", async () => {
    const pairing = pairingLink();
    const store = storeOfLink(pairing);
    expect(hostStoreOf(MOCK_HOST_KEY)).not.toBe(store);

    await pairing.client.connect({ host: "mock", port: DEFAULT_PORT });

    expect(hostStoreOf(MOCK_HOST_KEY)).toBe(store);
    // It mirrors its own link's connection, nobody else's.
    expect(store.getState().connection).toBe("connected");
    pairing.client.disconnect();
    expect(store.getState().connection).toBe("disconnected");
  });
});

describe("two hosts with the same agent id", () => {
  it("hold their own state, and a write to one never reaches the other", async () => {
    const [storeA, a, storeB, b] = twoHosts();
    vi.spyOn(a.api, "getGitState").mockResolvedValue(git("on-a"));
    vi.spyOn(b.api, "getGitState").mockResolvedValue(git("on-b"));

    await storeA.getState().loadGitState(AGENT);
    expect(storeA.getState().gitStates[AGENT]).toEqual(git("on-a"));
    expect(storeB.getState().gitStates[AGENT]).toBeUndefined();

    await storeB.getState().loadGitState(AGENT);
    storeA.setState({ logs: { [AGENT]: [{ kind: "user_message", text: "a", turnId: "t" }] } });
    expect(storeA.getState().gitStates[AGENT]).toEqual(git("on-a"));
    expect(storeB.getState().gitStates[AGENT]).toEqual(git("on-b"));
    expect(storeB.getState().logs[AGENT]).toBeUndefined();
  });

  it("clears one without touching the other", () => {
    const [storeA, , storeB] = twoHosts();
    storeA.setState({ workspace: workspace("a") });
    storeB.setState({ workspace: workspace("b") });
    storeB.getState().clear();
    expect(storeB.getState().workspace).toBeNull();
    expect(storeA.getState().workspace).toEqual(workspace("a"));
  });
});

describe("the order of one host's reads is its own", () => {
  it("a git read out on one host is not joined by the other's", async () => {
    const [storeA, a, storeB, b] = twoHosts();
    const slow = held<GitState>();
    vi.spyOn(a.api, "getGitState").mockReturnValue(slow.promise);
    const readB = vi.spyOn(b.api, "getGitState").mockResolvedValue(git("b-now"));

    const outA = storeA.getState().loadGitState(AGENT);
    await storeB.getState().loadGitState(AGENT);
    expect(readB).toHaveBeenCalledOnce();
    expect(storeB.getState().gitStates[AGENT]).toEqual(git("b-now"));

    slow.release(git("a-late"));
    await outA;
    expect(storeA.getState().gitStates[AGENT]).toEqual(git("a-late"));
  });

  it("one host's PR fence does not refuse the other's read", async () => {
    const [storeA, a, storeB] = twoHosts();
    const live = held<{ state: PrState; checks: null }>();
    vi.spyOn(a.api, "getPrLive").mockReturnValue(live.promise as never);

    const read = storeA.getState().loadPrLive(AGENT);
    // Clearing a store refuses every PR read it has out — its own.
    storeB.getState().clear();
    live.release({ state: pr("from a"), checks: null });
    await read;
    expect(storeA.getState().prStates[AGENT]).toEqual(pr("from a"));
  });

  it("one host's history read does not supersede the other's", async () => {
    const [storeA, a, storeB, b] = twoHosts();
    const page = held<{ records: never[]; older: null }>();
    vi.spyOn(a.api, "readSessionPage").mockReturnValue(page.promise as never);
    vi.spyOn(b.api, "readSessionPage").mockResolvedValue({ records: [], older: null } as never);
    for (const { api } of [a, b]) {
      vi.spyOn(api, "readUserTurns").mockResolvedValue([]);
      vi.spyOn(api, "syncSession").mockResolvedValue(null);
    }

    const readA = storeA.getState().rebuildLog(AGENT);
    await storeB.getState().rebuildLog(AGENT);
    expect(storeB.getState().logLoads[AGENT]).toEqual({ status: "ready" });
    expect(storeA.getState().logLoads[AGENT]).toEqual({ status: "loading" });

    page.release({ records: [], older: null });
    await readA;
    expect(storeA.getState().logLoads[AGENT]).toEqual({ status: "ready" });
  });

  it("one host's workspace read does not supersede the other's", async () => {
    const [storeA, a, storeB, b] = twoHosts();
    const slow = held<Workspace>();
    vi.spyOn(a.api, "getWorkspace").mockReturnValue(slow.promise);
    vi.spyOn(b.api, "getWorkspace").mockResolvedValue(workspace("b"));

    const readA = storeA.getState().refreshWorkspace();
    await storeB.getState().refreshWorkspace();
    slow.release(workspace("a"));
    await readA;
    expect(storeA.getState().workspace).toEqual(workspace("a"));
    expect(storeB.getState().workspace).toEqual(workspace("b"));
  });

  it("an approval event on one host is not replayed over the other's list", async () => {
    const [storeA, a, storeB] = twoHosts();
    const list = held<PublishApproval[]>();
    vi.spyOn(a.api, "listPublishApprovals").mockReturnValue(list.promise);
    const onB = { id: "on-b" } as PublishApproval;

    const loadA = storeA.getState().loadPendingApprovals();
    storeB.getState().receivePublishApproval(onB);
    list.release([]);
    await loadA;
    expect(storeA.getState().pendingPublishApprovals).toEqual([]);
    expect(storeB.getState().pendingPublishApprovals).toEqual([onB]);
  });
});
