import { describe, expect, it, vi } from "vitest";
import type { ProjectSettingsChangedEvent } from "../api/types/settings";
import { followProjectSettings, replaySettingChanges } from "./settingsReplay";

type Settings = Record<string, string>;

/** A source whose reads the test answers by hand, and whose stream it drives. */
function fakeSource() {
  const reads: { projectId: string; resolve: (s: Settings) => void }[] = [];
  const listeners = new Set<(e: ProjectSettingsChangedEvent) => void>();
  const off = vi.fn();
  return {
    reads,
    listeners,
    off,
    emit: (e: ProjectSettingsChangedEvent) => {
      for (const cb of listeners) cb(e);
    },
    source: {
      read: (projectId: string) =>
        new Promise<Settings>((resolve) => reads.push({ projectId, resolve })),
      subscribe: (cb: (e: ProjectSettingsChangedEvent) => void) => {
        listeners.add(cb);
        return Promise.resolve(() => {
          listeners.delete(cb);
          off();
        });
      },
    },
  };
}

/** A `setState` stand-in: folds every update over the last value. */
function state() {
  let value: Settings | null = null;
  return {
    update: (fold: (prev: Settings | null) => Settings) => {
      value = fold(value);
    },
    get: () => value,
  };
}

const flush = () => new Promise((r) => setTimeout(r, 0));

describe("replaySettingChanges", () => {
  it("applies writes in order over the snapshot, deletes included", () => {
    expect(
      replaySettingChanges({ a: "1", b: "2" }, [
        { key: "a", value: "3" },
        { key: "b", value: null },
        { key: "a", value: "4" },
      ]),
    ).toEqual({ a: "4" });
  });
});

describe("followProjectSettings", () => {
  it("subscribes before it reads", async () => {
    const f = fakeSource();
    followProjectSettings("p1", f.source, state().update);
    expect(f.listeners.size).toBe(1);
    await flush();
    expect(f.reads.map((r) => r.projectId)).toEqual(["p1"]);
  });

  it("replays an event that raced the read over its stale answer", async () => {
    const f = fakeSource();
    const s = state();
    followProjectSettings("p1", f.source, s.update);
    await flush();

    // Another client writes after the host served the read; its event lands first.
    f.emit({ project_id: "p1", key: "verify_cmd", value: "new" });
    f.emit({ project_id: "p1", key: "gone", value: null });
    expect(s.get()).toBeNull();

    f.reads[0].resolve({ verify_cmd: "old", gone: "x", kept: "y" });
    await flush();
    expect(s.get()).toEqual({ verify_cmd: "new", kept: "y" });

    // Once loaded, events fold straight in.
    f.emit({ project_id: "p1", key: "kept", value: "z" });
    expect(s.get()).toEqual({ verify_cmd: "new", kept: "z" });
  });

  it("ignores events for another project", async () => {
    const f = fakeSource();
    const s = state();
    followProjectSettings("p1", f.source, s.update);
    await flush();

    f.emit({ project_id: "p2", key: "verify_cmd", value: "theirs" });
    f.reads[0].resolve({ verify_cmd: "mine" });
    await flush();
    f.emit({ project_id: "p2", key: "verify_cmd", value: "theirs" });
    expect(s.get()).toEqual({ verify_cmd: "mine" });
  });

  it("drops a read superseded by a newer load, and unsubscribes", async () => {
    const f = fakeSource();
    const s = state();
    const stop = followProjectSettings("p1", f.source, s.update);
    await flush();
    stop();
    await flush();
    expect(f.off).toHaveBeenCalledOnce();

    followProjectSettings("p2", f.source, s.update);
    await flush();
    f.reads[1].resolve({ verify_cmd: "p2" });
    await flush();
    // The first load's answer lands last and must not overwrite the second's.
    f.reads[0].resolve({ verify_cmd: "p1" });
    await flush();
    expect(s.get()).toEqual({ verify_cmd: "p2" });

    // Nor does the stopped load hear events any more.
    f.emit({ project_id: "p1", key: "verify_cmd", value: "late" });
    expect(s.get()).toEqual({ verify_cmd: "p2" });
  });

  it("writes nothing when the read fails", async () => {
    const f = fakeSource();
    const s = state();
    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    followProjectSettings(
      "p1",
      { ...f.source, read: () => Promise.reject(new Error("unsupported")) },
      s.update,
    );
    await flush();
    f.emit({ project_id: "p1", key: "verify_cmd", value: "x" });
    expect(s.get()).toBeNull();
    expect(error).toHaveBeenCalled();
    error.mockRestore();
  });
});
