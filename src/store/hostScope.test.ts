// @vitest-environment happy-dom
// Every connected host's state is live at once: this Mac's at the store's top
// level, as it always was, and each paired host's in its own partition, written
// only through that host's scope. These pin the routing — what a scoped write
// lands where, that two hosts' same-named agents never meet, that a background
// host's events land in its own partition — and that forgetting a host takes
// everything of it away, and that this Mac's path is the store itself.

import { createElement, type ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

/** Every op this Mac's engine is asked, by name: the store's own `api` is
 *  whatever `@/api` exports, so a proxy there records them all. */
const { localCalls } = vi.hoisted(() => ({ localCalls: [] as string[] }));
vi.mock("@/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/api")>();
  const api = new Proxy(
    {},
    {
      get: (_target, op) => () => {
        localCalls.push(String(op));
        return Promise.resolve(undefined);
      },
    },
  );
  return { ...actual, api };
});

import type { AgentRecord, Workspace } from "@/api";
import { useEngine } from "@/api/engine";
import type { Transport } from "@/api/transport";
import { hostStore, startHost, stopHost, useAppStore, useUnseenCount } from "@/store";
import { renderHook } from "@/test/render";
import { EngineScope } from "./EngineScope";
import { LOCAL_ENVIRONMENT_ID } from "./environments";
import type { AppState } from "./types";

/** A paired host behind a fake connection: what it was asked, what is
 *  subscribed, and a way to raise one of its events. */
function fakeHost(id: string) {
  const calls: string[] = [];
  const handlers = new Map<string, Set<(payload: unknown) => void>>();
  const transport: Transport = {
    call: <T>(op: string): Promise<T> => {
      calls.push(op);
      return Promise.resolve((op === "get_workspace" ? workspace(id) : undefined) as T);
    },
    on: <T>(event: string, cb: (payload: T) => void) => {
      const set = handlers.get(event) ?? new Set();
      handlers.set(event, set);
      const handler = cb as (payload: unknown) => void;
      set.add(handler);
      return Promise.resolve(() => {
        set.delete(handler);
      });
    },
  };
  return {
    id,
    calls,
    transport,
    emit: (event: string, payload: unknown) => {
      for (const cb of handlers.get(event) ?? []) cb(payload);
    },
    listening: () => [...handlers.values()].reduce((n, set) => n + set.size, 0),
  };
}

type FakeHost = ReturnType<typeof fakeHost>;

/** A snapshot whose one agent is called `fuji`, on whichever host. */
const workspace = (label: string): Workspace =>
  ({
    repos: [],
    projects: [],
    agents: [{ id: "fuji", name: label, status: "idle", repos: [] } as unknown as AgentRecord],
  }) as Workspace;

/** Publish a host the way the registry does as its client is created, and
 *  make it live. */
async function publish(host: FakeHost) {
  useAppStore.getState().upsertEnvironment({
    id: host.id,
    name: host.id,
    kind: "remote",
    connection: "connected",
    transport: host.transport,
  });
  await startHost(host.id);
}

/** Forget a host the way the registry does. */
async function forget(id: string) {
  await stopHost(id);
  useAppStore.getState().removeEnvironment(id);
}

const settle = async () => {
  for (let i = 0; i < 20; i++) await Promise.resolve();
};

const partition = (id: string) => useAppStore.getState().hostStates[id];

let hosts: FakeHost[] = [];

beforeEach(async () => {
  localCalls.length = 0;
  useAppStore.setState(useAppStore.getInitialState(), true);
  hosts = [fakeHost("studio"), fakeHost("cloud")];
  for (const host of hosts) await publish(host);
});

afterEach(async () => {
  for (const host of hosts) await forget(host.id);
});

describe("a host's scoped set", () => {
  it("writes the host's keys into its partition and everything else at the top level", () => {
    const studio = hostStore("studio");

    studio.setState({ selectedAgentId: "fuji", lastError: "studio said no" });

    expect(partition("studio").selectedAgentId).toBe("fuji");
    expect(useAppStore.getState().lastError).toBe("studio said no");
    // This Mac's own copy of the host key is untouched…
    expect(useAppStore.getState().selectedAgentId).toBeNull();
    // …and no top-level key was written for the host's.
    expect(useAppStore.getState().hostStates.studio).not.toHaveProperty("lastError");
    expect(studio.getState().selectedAgentId).toBe("fuji");
  });

  it("runs an update against the host's view", () => {
    const studio = hostStore("studio");
    studio.setState({ unseenResults: { fuji: true } });

    studio.setState((s) => ({ unseenResults: { ...s.unseenResults, kyoto: true } }));

    expect(partition("studio").unseenResults).toEqual({ fuji: true, kyoto: true });
    expect(useAppStore.getState().unseenResults).toEqual({});
  });

  it("writes nothing for an update that hands the view back unchanged", () => {
    const before = useAppStore.getState();

    hostStore("studio").setState((s) => s);

    expect(useAppStore.getState()).toBe(before);
  });

  it("gives the same view while nothing changed", () => {
    const studio = hostStore("studio");
    expect(studio.getState()).toBe(studio.getState());
  });
});

describe("two hosts with an agent of the same name", () => {
  it("never write each other's state, nor this Mac's", () => {
    // `fuji` is on this Mac and on both hosts, each with a result unseen.
    useAppStore.setState({ unseenResults: { fuji: true } });
    hostStore("studio").setState({ unseenResults: { fuji: true } });
    hostStore("cloud").setState({ unseenResults: { fuji: true } });

    // Opening the studio's `fuji` — through the studio's own action.
    hostStore("studio").getState().selectAgent("fuji");

    expect(partition("studio").selectedAgentId).toBe("fuji");
    expect(partition("studio").unseenResults).toEqual({});
    expect(partition("cloud").selectedAgentId).toBeNull();
    expect(partition("cloud").unseenResults).toEqual({ fuji: true });
    expect(useAppStore.getState().selectedAgentId).toBeNull();
    expect(useAppStore.getState().unseenResults).toEqual({ fuji: true });

    // And this Mac's, through the store's own.
    useAppStore.getState().selectAgent("fuji");
    expect(useAppStore.getState().unseenResults).toEqual({});
    expect(partition("cloud").unseenResults).toEqual({ fuji: true });
  });

  it("read each from its own engine", async () => {
    await hostStore("studio").getState().loadHistoryTranscript("fuji");

    expect(hosts[0].calls).toContain("read_session_records");
    expect(hosts[1].calls).toEqual([]);
    expect(localCalls).toEqual([]);
    expect(partition("studio").transcriptLoaded).toEqual({ fuji: true });
    expect(partition("cloud").transcriptLoaded).toEqual({});
    expect(useAppStore.getState().transcriptLoaded).toEqual({});
  });
});

describe("a host's events", () => {
  it("fold into its own partition while another host is on screen", () => {
    expect(useAppStore.getState().activeEnvironmentId).toBe(LOCAL_ENVIRONMENT_ID);

    hosts[1].emit("turn:started", { agent_id: "fuji", started_at: 42 });

    expect(partition("cloud").turnStartedAt).toEqual({ fuji: 42 });
    expect(partition("studio").turnStartedAt).toEqual({});
    expect(useAppStore.getState().turnStartedAt).toEqual({});
  });

  it("go on folding after a switch, which asks no engine anything", async () => {
    useAppStore.setState({ selectedAgentId: "kyoto" });
    hostStore("studio").setState({ selectedAgentId: "fuji" });
    const studioCalls = hosts[0].calls.length;

    useAppStore.getState().switchEnvironment("studio");

    expect(useAppStore.getState().activeEnvironmentId).toBe("studio");
    // Both hosts' state is where it was.
    expect(useAppStore.getState().selectedAgentId).toBe("kyoto");
    expect(partition("studio").selectedAgentId).toBe("fuji");

    // The host left behind keeps updating.
    hosts[1].emit("turn:started", { agent_id: "fuji", started_at: 7 });
    expect(partition("cloud").turnStartedAt).toEqual({ fuji: 7 });

    await settle();
    expect(hosts[0].calls).toHaveLength(studioCalls);
    expect(localCalls).toEqual([]);
  });
});

describe("a turn that ends on a host", () => {
  /** Claude's own word that the turn is over, as the host forwards it. */
  const turnEnds = (host: FakeHost) =>
    host.emit("agent:event", { agent_id: "fuji", event: { type: "result", subtype: "success" } });

  it("is unseen when its host is in the background, even for the agent selected there", () => {
    hostStore("cloud").setState({ workspace: workspace("cloud"), selectedAgentId: "fuji" });

    turnEnds(hosts[1]);

    expect(partition("cloud").unseenResults).toEqual({ fuji: true });
  });

  it("is seen when its host is on screen with that agent selected", () => {
    hostStore("cloud").setState({ workspace: workspace("cloud"), selectedAgentId: "fuji" });
    useAppStore.getState().switchEnvironment("cloud");

    turnEnds(hosts[1]);

    expect(partition("cloud").unseenResults).toEqual({});
  });

  it("is counted for the window whichever host is on screen, until it is seen", () => {
    hostStore("cloud").setState({ workspace: workspace("cloud"), selectedAgentId: "fuji" });
    useAppStore.setState({ unseenResults: { kyoto: true } });
    turnEnds(hosts[1]);
    const inStudio = ({ children }: { children: ReactNode }) =>
      createElement(EngineScope, { hostId: "studio", children });

    const { result, rerender } = renderHook(() => useUnseenCount(), { wrapper: inStudio });
    expect(result.current).toBe(2);

    hostStore("cloud").getState().markResultsSeen("fuji");
    rerender();
    expect(result.current).toBe(1);
    expect(partition("cloud").unseenResults).toEqual({});
  });
});

describe("a handshake", () => {
  it("re-reads the host's own partition whether or not it is on screen", async () => {
    hostStore("cloud").getState().environmentReconnected("cloud");
    await settle();

    expect(hosts[1].calls).toContain("get_workspace");
    expect(partition("cloud").workspace?.agents[0].name).toBe("cloud");
    expect(partition("studio").workspace).toBeNull();
    expect(useAppStore.getState().workspace).toBeNull();
    expect(localCalls).not.toContain("getWorkspace");
  });
});

describe("forgetting a host", () => {
  it("drops its partition, its actions and its listeners", async () => {
    const studio = hosts[0];
    const before = hostStore("studio").getState().selectAgent;
    expect(studio.listening()).toBeGreaterThan(0);

    await forget("studio");

    expect(useAppStore.getState().hostStates.studio).toBeUndefined();
    expect(studio.listening()).toBe(0);
    // An event that still reaches it lands nowhere.
    studio.emit("turn:started", { agent_id: "fuji", started_at: 1 });
    expect(useAppStore.getState().hostStates.studio).toBeUndefined();
    expect(useAppStore.getState().turnStartedAt).toEqual({});

    // Paired again, it starts over: a blank partition and actions of its own.
    await publish(studio);
    expect(partition("studio").selectedAgentId).toBeNull();
    expect(hostStore("studio").getState().selectAgent).not.toBe(before);
  });
});

describe("useAppStore in a scope", () => {
  it("hands this Mac's selectors the store's own state", () => {
    const seen: AppState[] = [];

    renderHook(() =>
      useAppStore((s) => {
        seen.push(s);
        return s.workspace;
      }),
    );

    expect(seen.length).toBeGreaterThan(0);
    for (const state of seen) expect(state).toBe(useAppStore.getState());
  });

  it("hands a host's selectors its view, beside its engine", () => {
    hostStore("studio").setState({ selectedAgentId: "fuji" });
    const inStudio = ({ children }: { children: ReactNode }) =>
      createElement(EngineScope, { hostId: "studio", children });

    const { result } = renderHook(
      () => ({
        selected: useAppStore((s) => s.selectedAgentId),
        hostId: useEngine().hostId,
      }),
      { wrapper: inStudio },
    );

    expect(result.current).toEqual({ selected: "fuji", hostId: "studio" });
  });
});
