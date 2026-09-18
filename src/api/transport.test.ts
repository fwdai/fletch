// `RemoteTransport` against a fake protocol client: the adapter is all this
// class is, so what matters is that ops and args arrive unchanged, events are
// delivered as bare payloads (no Tauri envelope), and the unsubscribe the
// client hands back is the one the caller gets.

import { describe, expect, it } from "vitest";
import type { EventHandler } from "@/remote/types";
import { RemoteTransport } from "./transport";

function fakeClient() {
  const calls: { op: string; args?: Record<string, unknown> }[] = [];
  const handlers = new Map<string, Set<EventHandler>>();
  return {
    calls,
    emit(event: string, payload: unknown) {
      for (const cb of handlers.get(event) ?? []) cb(payload);
    },
    subscribed: (event: string) => handlers.get(event)?.size ?? 0,
    call<T>(op: string, args?: Record<string, unknown>): Promise<T> {
      calls.push({ op, args });
      return Promise.resolve(`${op}-result` as T);
    },
    on(event: string, cb: EventHandler): () => void {
      const set = handlers.get(event) ?? new Set<EventHandler>();
      set.add(cb);
      handlers.set(event, set);
      return () => {
        set.delete(cb);
      };
    },
  };
}

describe("RemoteTransport", () => {
  it("forwards a call with its args and resolves the client's result", async () => {
    const client = fakeClient();
    const transport = new RemoteTransport(client);

    await expect(transport.call<string>("spawn_agent", { repoPath: "/repos/app" })).resolves.toBe(
      "spawn_agent-result",
    );
    expect(client.calls).toEqual([{ op: "spawn_agent", args: { repoPath: "/repos/app" } }]);
  });

  it("sends an empty args object when the caller passes none", async () => {
    const client = fakeClient();
    await new RemoteTransport(client).call("get_workspace");
    expect(client.calls).toEqual([{ op: "get_workspace", args: {} }]);
  });

  it("delivers event payloads and unsubscribes", async () => {
    const client = fakeClient();
    const transport = new RemoteTransport(client);
    const seen: string[] = [];

    const unlisten = await transport.on<{ agent_id: string }>("agent:status", (p) =>
      seen.push(p.agent_id),
    );
    client.emit("agent:status", { agent_id: "fuji" });
    expect(seen).toEqual(["fuji"]);
    expect(client.subscribed("agent:status")).toBe(1);

    unlisten();
    client.emit("agent:status", { agent_id: "kyoto" });
    expect(seen).toEqual(["fuji"]);
    expect(client.subscribed("agent:status")).toBe(0);
  });
});
