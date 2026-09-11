// The install state machine's transitions. These pin the rules that decide
// what a provider row shows, all of which were bugs waiting to happen:
//
//   1. An installer exiting 0 is NOT proof of an install — if the re-probe
//      still can't find the binary, the row must say so rather than claim
//      success the model picker will then contradict.
//   2. Only a live run can be transitioned. A cancelled run's own terminal
//      event (and the trailing output lines its pipes were still draining)
//      arrive *after* the row went back to idle, and must not resurrect it.
//   3. Installer output is unbounded; the retained log is not.
//   4. A cancel is not done when the click lands — it is done when the
//      backend has torn the installer's process group down. Until then the row
//      must not offer Install or Retry, or the "retry" races the install it is
//      replacing.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { create } from "zustand";

const { cancelAgentInstall, installAgent } = vi.hoisted(() => ({
  cancelAgentInstall: vi.fn(),
  installAgent: vi.fn(),
}));
vi.mock("@/api", () => ({ api: { cancelAgentInstall, installAgent } }));
vi.mock("@/util/track", () => ({ track: vi.fn() }));

import {
  applyInstallDone,
  createAgentInstallSlice,
  INSTALL_LOG_CAP,
  type InstallState,
  NOT_ON_PATH_ERROR,
  reduceInstallEvent,
} from "./agentInstall";
import type { AppState } from "./types";

const running = (log: string[] = [], line?: string): InstallState => ({
  phase: "running",
  line,
  log,
});

const cancelling = (log: string[] = []): InstallState => ({ phase: "cancelling", log });

const makeStore = (installs: Record<string, InstallState>) => {
  const store = create<AppState>()((...a) => ({ ...createAgentInstallSlice(...a) }) as AppState);
  store.setState({ installs });
  return store;
};

describe("reduceInstallEvent", () => {
  it("appends output lines to the live run", () => {
    const next = reduceInstallEvent(
      { claude: running(["$ curl … | bash"]) },
      { id: "claude", phase: "running", line: "downloading" },
    );
    expect(next.claude).toEqual({
      phase: "running",
      line: "downloading",
      log: ["$ curl … | bash", "downloading"],
    });
  });

  it("keeps the last line when an event carries none", () => {
    const next = reduceInstallEvent(
      { claude: running(["a"], "a") },
      { id: "claude", phase: "running" },
    );
    expect(next.claude).toEqual({ phase: "running", line: "a", log: ["a"] });
  });

  it("caps the retained log", () => {
    const log = Array.from({ length: INSTALL_LOG_CAP }, (_, i) => `line ${i}`);
    const next = reduceInstallEvent(
      { claude: running(log) },
      { id: "claude", phase: "running", line: "newest" },
    );
    const kept = next.claude as Extract<InstallState, { phase: "running" }>;
    expect(kept.log).toHaveLength(INSTALL_LOG_CAP);
    expect(kept.log.at(0)).toBe("line 1");
    expect(kept.log.at(-1)).toBe("newest");
  });

  it("fails the run with the installer's own error, keeping the log", () => {
    const next = reduceInstallEvent(
      { claude: running(["$ curl … | bash", "curl: (6) could not resolve host"]) },
      { id: "claude", phase: "failed", error: "installer exited with exit status: 1" },
    );
    expect(next.claude).toEqual({
      phase: "failed",
      error: "installer exited with exit status: 1",
      log: ["$ curl … | bash", "curl: (6) could not resolve host"],
    });
  });

  it("takes a cancelled run back to idle", () => {
    const next = reduceInstallEvent(
      { claude: running(["a"]), codex: running([]) },
      {
        id: "claude",
        phase: "cancelled",
      },
    );
    expect(next).toEqual({ codex: running([]) });
  });

  it("ignores late events for a run that is no longer live", () => {
    // Cancel already cleared the row: the drained output line and the
    // installer's terminal event must not bring it back.
    for (const event of [
      { id: "claude", phase: "running", line: "still downloading" },
      { id: "claude", phase: "failed", error: "killed" },
      { id: "claude", phase: "cancelled" },
    ] as const) {
      expect(reduceInstallEvent({}, event)).toEqual({});
    }
  });

  it("leaves a settled row alone", () => {
    const settled = { claude: { phase: "fresh" } as InstallState };
    expect(reduceInstallEvent(settled, { id: "claude", phase: "failed", error: "late" })).toBe(
      settled,
    );
  });

  it("does not settle `done` itself — that waits on the re-probe", () => {
    const live = { claude: running(["a"]) };
    expect(reduceInstallEvent(live, { id: "claude", phase: "done" })).toBe(live);
  });
});

describe("applyInstallDone", () => {
  it("marks a detected binary as freshly installed", () => {
    expect(applyInstallDone({ claude: running(["a"]) }, "claude", true)).toEqual({
      claude: { phase: "fresh" },
    });
  });

  it("fails a run whose binary never appeared on PATH", () => {
    expect(applyInstallDone({ claude: running(["a"]) }, "claude", false)).toEqual({
      claude: { phase: "failed", error: NOT_ON_PATH_ERROR, log: ["a"] },
    });
  });

  it("ignores a run the user already cancelled", () => {
    expect(applyInstallDone({}, "claude", true)).toEqual({});
  });

  it("ignores a run that is still being torn down", () => {
    const stopping = { claude: cancelling(["a"]) };
    expect(applyInstallDone(stopping, "claude", true)).toBe(stopping);
  });
});

describe("a run that is being cancelled", () => {
  beforeEach(() => vi.clearAllMocks());

  it("ignores everything the dying installer still emits", () => {
    const stopping = { claude: cancelling(["$ curl … | bash"]) };
    // Output the killed child's pipes were still draining, and the "failed"
    // its non-zero exit produces — neither is news to a user who asked it to
    // stop, and neither may put Retry back on the row.
    for (const event of [
      { id: "claude", phase: "running", line: "still downloading" },
      { id: "claude", phase: "failed", error: "installer exited with signal: 1" },
    ] as const) {
      expect(reduceInstallEvent(stopping, event)).toBe(stopping);
    }
    // Its own terminal event is the same answer the awaited cancel gives, so
    // whichever lands first the row ends up idle.
    expect(reduceInstallEvent(stopping, { id: "claude", phase: "cancelled" })).toEqual({});
  });

  it("stays on the row until the backend confirms the teardown", async () => {
    let confirm!: (torn: boolean) => void;
    cancelAgentInstall.mockReturnValueOnce(
      new Promise<boolean>((resolve) => {
        confirm = resolve;
      }),
    );
    const store = makeStore({ claude: running(["$ curl … | bash", "downloading"]) });

    const cancelled = store.getState().cancelAgentInstall("claude");
    // The installer's process group is still dying: the log stays up, and the
    // row is emphatically not back to "missing" with an Install button.
    expect(store.getState().installs.claude).toEqual(
      cancelling(["$ curl … | bash", "downloading"]),
    );

    confirm(true);
    await cancelled;
    expect(store.getState().installs).toEqual({});
  });

  it("refuses to start a second install before the first is gone", async () => {
    const store = makeStore({ claude: cancelling(["a"]) });
    await store.getState().installAgent("claude");
    // The backend still holds the agent's slot, so this could only have earned
    // an "already in progress" error.
    expect(installAgent).not.toHaveBeenCalled();
    expect(store.getState().installs.claude).toEqual(cancelling(["a"]));
  });

  it("clears the row even when the cancel round trip fails", async () => {
    cancelAgentInstall.mockRejectedValueOnce(new Error("ipc exploded"));
    const logged = vi.spyOn(console, "error").mockImplementation(() => {});
    const store = makeStore({ claude: running(["a"]) });

    // The backend frees the slot on every exit path, so a lost answer must not
    // strand the row at "cancelling" with no way out.
    await store.getState().cancelAgentInstall("claude");
    expect(store.getState().installs).toEqual({});
    expect(logged).toHaveBeenCalled();
    logged.mockRestore();
  });
});
