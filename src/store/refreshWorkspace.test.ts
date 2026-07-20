// Regression tests for the workspace-refresh generation guard. The bug this
// pins: deleting several agents in a row leaves multiple `getWorkspace()`
// fetches in flight, and an older snapshot resolving last used to clobber a
// newer one — flashing a just-deleted agent back into the sidebar. The guard
// must drop any fetch that a newer refresh has superseded.

import { beforeEach, describe, expect, it, vi } from "vitest";

const { getWorkspace } = vi.hoisted(() => ({ getWorkspace: vi.fn() }));
vi.mock("@/api", () => ({ api: { getWorkspace } }));

import { refreshWorkspace } from "./refreshWorkspace";

interface Deferred<T> {
  promise: Promise<T>;
  resolve: (value: T) => void;
}
const defer = <T>(): Deferred<T> => {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((r) => {
    resolve = r;
  });
  return { promise, resolve };
};

// Minimal workspace shape — the guard only ever touches `.agents`.
const ws = (agentIds: string[]) =>
  ({ agents: agentIds.map((id) => ({ id })) }) as unknown as Awaited<
    ReturnType<typeof getWorkspace>
  >;

// biome-ignore lint/suspicious/noExplicitAny: test set() stub inspects the updater's output
const agentIds = (patch: any): string[] => patch.workspace.agents.map((a: { id: string }) => a.id);

describe("refreshWorkspace generation guard", () => {
  beforeEach(() => {
    getWorkspace.mockReset();
  });

  it("drops a stale in-flight snapshot so it can't clobber a newer one", async () => {
    const stale = defer<unknown>();
    const fresh = defer<unknown>();
    getWorkspace.mockReturnValueOnce(stale.promise).mockReturnValueOnce(fresh.promise);

    // biome-ignore lint/suspicious/noExplicitAny: test stub
    const applied: any[] = [];
    // biome-ignore lint/suspicious/noExplicitAny: test stub for the store set()
    const set = (updater: any) => {
      applied.push(updater({}));
    };

    // Two refreshes issued back-to-back; the second is the up-to-date snapshot
    // (agent "a" was deleted), the first still shows "a".
    const stalePending = refreshWorkspace(set);
    const freshPending = refreshWorkspace(set);

    // The newer (later-issued) fetch resolves first and applies...
    fresh.resolve(ws([]));
    await freshPending;
    // ...then the older fetch resolves late and must be discarded.
    stale.resolve(ws(["a"]));
    await stalePending;

    expect(applied).toHaveLength(1);
    expect(agentIds(applied[0])).toEqual([]);
  });

  it("applies only the latest generation and returns null for the superseded one", async () => {
    getWorkspace.mockResolvedValueOnce(ws(["a"])).mockResolvedValueOnce(ws(["b"]));
    const set = vi.fn();

    const [superseded, latest] = await Promise.all([refreshWorkspace(set), refreshWorkspace(set)]);

    expect(superseded).toBeNull();
    expect(latest?.agents.map((a) => a.id)).toEqual(["b"]);
    expect(set).toHaveBeenCalledTimes(1);
  });
});
