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

import { describe, expect, it, vi } from "vitest";

// The slice's actions talk to the backend; its transitions — all this file
// exercises — are pure.
vi.mock("@/api", () => ({ api: {} }));

import {
  applyInstallDone,
  INSTALL_LOG_CAP,
  type InstallState,
  NOT_ON_PATH_ERROR,
  reduceInstallEvent,
} from "./agentInstall";

const running = (log: string[] = [], line?: string): InstallState => ({
  phase: "running",
  line,
  log,
});

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
});
