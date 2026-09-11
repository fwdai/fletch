// One-click agent CLI installs, shared by every surface that offers them
// (onboarding's agents step and Settings › Providers). The backend runs the
// pinned vendor installer (src-tauri/src/agent_install.rs) and streams its
// output as `agent-install:state` events; this slice is the single place that
// folds those into UI state, so the two surfaces can't drift — or register two
// listeners for the same stream.
//
// The terminal `done` event is deliberately NOT applied by the reducer: an
// installer exiting 0 doesn't prove a binary landed on PATH, so `installAgent`
// re-probes first and only then decides "fresh" vs "failed".

import { type AgentInstallEvent, api } from "@/api";
import { track } from "@/util/track";
import type { SliceCreator } from "./types";

/** Per-agent install progress. Absent = idle (nothing running, nothing to
 *  report) — which is also where a cancelled run lands. */
export type InstallState =
  | { phase: "running"; line?: string; log: string[] }
  | { phase: "failed"; error: string; log: string[] }
  | { phase: "fresh" };

/** Installer output retained per run. Enough to see what went wrong in a
 *  failure, bounded so a chatty progress bar can't grow without limit. */
export const INSTALL_LOG_CAP = 200;

/** Shown when the installer exits 0 but the re-probe still can't find the
 *  binary — a real failure the user has to see, not a silent success. */
export const NOT_ON_PATH_ERROR = "installer finished but the binary was not found on PATH";

export interface AgentInstallSlice {
  /** Live install state keyed by provider id; absent means idle. */
  installs: Record<string, InstallState>;
  /** Run the pinned installer for an agent, then re-probe and settle the row.
   *  No-op while that agent is already installing. */
  installAgent: (id: string) => Promise<void>;
  /** Stop a running installer and put the row back to idle. */
  cancelAgentInstall: (id: string) => Promise<void>;
  /** Drop an agent's install state — dismissing a failure, or clearing the
   *  "Installed just now" flag once the user has seen it. */
  clearInstallState: (id: string) => void;
}

// Monotonic per-agent run ids. A cancel (or a retry) bumps the id, so the
// promise of the run it replaced can't land its result on the row afterwards.
// Module-level like `interrupted.ts`: bookkeeping nothing renders.
const runs = new Map<string, number>();
const startRun = (id: string): number => {
  const next = (runs.get(id) ?? 0) + 1;
  runs.set(id, next);
  return next;
};
const isCurrentRun = (id: string, run: number): boolean => runs.get(id) === run;

const appendLine = (log: string[], line: string | undefined): string[] =>
  line ? [...log, line].slice(-INSTALL_LOG_CAP) : log;

const without = (
  installs: Record<string, InstallState>,
  id: string,
): Record<string, InstallState> => {
  if (!(id in installs)) return installs;
  const { [id]: _gone, ...rest } = installs;
  return rest;
};

/** Fold one `agent-install:state` event into the install map.
 *
 *  Only a run this client started (phase "running") can be transitioned: a
 *  stray line or a late terminal event from a run the user already cancelled
 *  must not resurrect its row. `done` is a no-op here — see the module note. */
export function reduceInstallEvent(
  installs: Record<string, InstallState>,
  e: AgentInstallEvent,
): Record<string, InstallState> {
  const prev = installs[e.id];
  switch (e.phase) {
    case "running":
      if (prev?.phase !== "running") return installs;
      return {
        ...installs,
        [e.id]: { phase: "running", line: e.line ?? prev.line, log: appendLine(prev.log, e.line) },
      };
    case "failed":
      if (prev?.phase !== "running") return installs;
      return {
        ...installs,
        [e.id]: { phase: "failed", error: e.error ?? "installer failed", log: prev.log },
      };
    case "cancelled":
      return without(installs, e.id);
    default:
      return installs;
  }
}

/** Settle a finished run once the post-install re-probe has answered: a binary
 *  on PATH is a genuine success ("fresh"), an installer that exited 0 without
 *  leaving one is a failure. */
export function applyInstallDone(
  installs: Record<string, InstallState>,
  id: string,
  installed: boolean,
): Record<string, InstallState> {
  const prev = installs[id];
  if (prev?.phase !== "running") return installs;
  return {
    ...installs,
    [id]: installed
      ? { phase: "fresh" }
      : { phase: "failed", error: NOT_ON_PATH_ERROR, log: prev.log },
  };
}

export const createAgentInstallSlice: SliceCreator<AgentInstallSlice> = (set, get) => ({
  installs: {},

  installAgent: async (id) => {
    if (get().installs[id]?.phase === "running") return;
    const run = startRun(id);
    set((s) => ({ installs: { ...s.installs, [id]: { phase: "running", log: [] } } }));
    // Provider id only — the installer's error text is a raw shell message and
    // never leaves the machine.
    track("agent_cli_install_started", { provider: id });
    try {
      await api.installAgent(id);
      if (!isCurrentRun(id, run)) return;
      // `refreshProviderVersions` swallows its own errors and keeps the last
      // known-good probe, so a transient IPC failure reads as "not detected"
      // rather than throwing the successful install into the catch below.
      await get().refreshProviderVersions();
      if (!isCurrentRun(id, run)) return;
      const installed = !!get().providerPaths[id];
      // Success is "the binary is now detectable", not "the script exited 0".
      track(installed ? "agent_cli_install_succeeded" : "agent_cli_install_failed", {
        provider: id,
      });
      set((s) => ({ installs: applyInstallDone(s.installs, id, installed) }));
    } catch (err) {
      if (!isCurrentRun(id, run)) return;
      track("agent_cli_install_failed", { provider: id });
      set((s) => ({
        installs: reduceInstallEvent(s.installs, { id, phase: "failed", error: String(err) }),
      }));
    }
  },

  cancelAgentInstall: async (id) => {
    if (get().installs[id]?.phase !== "running") return;
    // Retire the run before anything can resolve onto the row, then go back to
    // idle immediately: the killed installer's own `cancelled` event lands
    // later (and is a no-op by then), but the click shouldn't wait for it.
    startRun(id);
    get().clearInstallState(id);
    await api.cancelAgentInstall(id).catch(() => {});
  },

  clearInstallState: (id) =>
    set((s) => {
      const installs = without(s.installs, id);
      return installs === s.installs ? {} : { installs };
    }),
});
