import { beforeEach, describe, expect, it, vi } from "vitest";
import { type BootStatus, waitForEngineReady } from "./boot";

const { invoke, listeners } = vi.hoisted(() => ({
  invoke: vi.fn(),
  listeners: [] as ((e: { payload: BootStatus }) => void)[],
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: (_event: string, cb: (e: { payload: BootStatus }) => void) => {
    listeners.push(cb);
    return Promise.resolve(() => {
      listeners.splice(listeners.indexOf(cb), 1);
    });
  },
}));

const emit = (payload: BootStatus) => {
  for (const cb of [...listeners]) cb({ payload });
};

describe("waitForEngineReady", () => {
  beforeEach(() => {
    invoke.mockReset();
    listeners.length = 0;
  });

  it("resolves at once when the engine was ready before it asked", async () => {
    invoke.mockResolvedValue({ phase: "ready" });
    const seen: BootStatus[] = [];

    await waitForEngineReady((s) => seen.push(s));

    expect(invoke).toHaveBeenCalledWith("boot_state");
    expect(seen).toEqual([{ phase: "ready" }]);
    expect(listeners).toHaveLength(0);
  });

  it("follows the phases over the event until ready arrives", async () => {
    invoke.mockResolvedValue({ phase: "booting", step: "backing_up_database" });
    const seen: BootStatus[] = [];
    const ready = waitForEngineReady((s) => seen.push(s));
    // The listener is registered before the query is answered.
    await vi.waitFor(() => expect(listeners).toHaveLength(1));
    await vi.waitFor(() => expect(seen).toHaveLength(1));

    emit({ phase: "booting", step: "migrating_database" });
    emit({ phase: "booting", step: "starting_engine" });
    emit({ phase: "ready" });

    await ready;
    expect(seen.map((s) => (s.phase === "booting" ? s.step : s.phase))).toEqual([
      "backing_up_database",
      "migrating_database",
      "starting_engine",
      "ready",
    ]);
    expect(listeners).toHaveLength(0);
  });

  it("rejects with the engine's message when boot failed", async () => {
    invoke.mockResolvedValue({ phase: "booting", step: "opening_database" });
    const ready = waitForEngineReady();
    await vi.waitFor(() => expect(listeners).toHaveLength(1));

    emit({ phase: "failed", message: "database init failed: disk full" });

    await expect(ready).rejects.toThrow("database init failed: disk full");
  });
});
