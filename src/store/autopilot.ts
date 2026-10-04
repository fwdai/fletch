// Autopilot as the host runs it (docs/remote-protocol.md, "Autopilot"). Every
// field here is a MIRROR: the host owns the loop, the switches and the history,
// and this window renders them and asks the host to flip a switch. Seeded by
// `autopilot_state {}` / `autopilot_log {}` on bootstrap, reconnect and
// environment switch (`loadAutopilot`); kept current by `autopilot:state`,
// `autopilot:switches` and `autopilot:event` in between — so every client
// driving the host shows the same thing at the same moment.

import {
  type AutopilotCheckout,
  type AutopilotLogEntry,
  type AutopilotSnapshot,
  type AutopilotSwitches,
  api,
} from "@/api";
import { hostSupports } from "@/remote/types";
import { newestWins } from "@/util/newestWins";
import { activeEnvironment, forActiveEnvironment } from "./environments";
import { gateReason } from "./gates";
import { checkoutKey } from "./git";
import type { SliceCreator } from "./types";

/** Rows kept per checkout — the host's own bound on `autopilot_log`. */
export const AUTOPILOT_LOG_LIMIT = 50;

/** The one answer to "is autopilot on for this project?", shared by the
 *  project switch and the Git panel's pause so they can never disagree.
 *  `disabled === null` means the opt-outs are not known yet; nothing reads as
 *  on until they are. */
export function autopilotProjectOn(disabled: readonly string[] | null, projectId: string): boolean {
  return disabled !== null && !disabled.includes(projectId);
}

export interface AutopilotSlice {
  /** The host's row per checkout, keyed by `checkoutKey(agent_id, subdir)`.
   *  Absent = the host reported nothing for it (yet). */
  autopilot: Record<string, AutopilotCheckout>;
  /** What autopilot did per checkout, keyed like `autopilot`, newest first. */
  autopilotLog: Record<string, AutopilotLogEntry[]>;
  /** Projects whose switch is off on the host; every other project is on.
   *  `null` until the first `autopilot_state` answers (or while it fails). */
  autopilotDisabledProjects: string[] | null;
  /** Agents paused from the Git panel's switch, on the host. */
  autopilotPausedAgents: string[];

  /** Replace the mirror with the host's state and history. */
  loadAutopilot: () => Promise<void>;
  /** Fold one `autopilot:state` into the mirror: the row, and the two lists. */
  applyAutopilotState: (row: AutopilotCheckout) => void;
  /** Take one `autopilot:switches`: both lists, replaced whole. */
  applyAutopilotSwitches: (switches: AutopilotSwitches) => void;
  /** Fold one `autopilot:event` into that checkout's history. */
  applyAutopilotEvent: (entry: AutopilotLogEntry) => void;
  /** Flip a project's switch on the host. */
  setProjectAutopilot: (projectId: string, enabled: boolean) => Promise<void>;
  /** Pause (or resume) one agent's checkouts on the host. */
  setAgentAutopilot: (agentId: string, enabled: boolean) => Promise<void>;
}

type Mirror = Pick<
  AutopilotSlice,
  "autopilot" | "autopilotDisabledProjects" | "autopilotPausedAgents"
>;
type Log = AutopilotSlice["autopilotLog"];
type SwitchTarget = { projectId: string } | { agentId: string };

const keyOf = (r: { agent_id: string; subdir: string | null }) =>
  checkoutKey(r.agent_id, r.subdir ?? undefined);

/** `list` with `id` in or out — the same array when nothing changes, so a
 *  selector over it does not re-render on every event. */
function withMember(list: string[], id: string, member: boolean): string[] {
  if (list.includes(id) === member) return list;
  return member ? [...list, id] : list.filter((x) => x !== id);
}

function foldRow(m: Mirror, row: AutopilotCheckout): Mirror {
  return {
    autopilot: { ...m.autopilot, [keyOf(row)]: row },
    autopilotPausedAgents: withMember(m.autopilotPausedAgents, row.agent_id, row.paused),
    // A row says nothing about the other projects, so unknown stays unknown.
    autopilotDisabledProjects:
      m.autopilotDisabledProjects &&
      withMember(m.autopilotDisabledProjects, row.project_id, !row.project_enabled),
  };
}

/** The host's lists, whole — idempotent, so a repeat or a late one is harmless
 *  and nothing is merged. Fills unknown opt-outs too. */
function foldSwitches(m: Mirror, switches: AutopilotSwitches): Mirror {
  return {
    autopilot: m.autopilot,
    autopilotDisabledProjects: switches.disabled_projects,
    autopilotPausedAgents: switches.paused_agents,
  };
}

function foldEntry(log: Log, entry: AutopilotLogEntry): Log {
  const key = keyOf(entry);
  const rows = log[key] ?? [];
  if (rows.some((r) => r.id === entry.id)) return log;
  return { ...log, [key]: [entry, ...rows].slice(0, AUTOPILOT_LOG_LIMIT) };
}

function mirrorOf(snapshot: AutopilotSnapshot): Mirror {
  const autopilot: Record<string, AutopilotCheckout> = {};
  for (const row of snapshot.checkouts) autopilot[keyOf(row)] = row;
  return {
    autopilot,
    autopilotDisabledProjects: snapshot.disabled_projects,
    autopilotPausedAgents: snapshot.paused_agents,
  };
}

/** Whether `target` is on in `m` — the value a failed write puts back. */
function switchOn(m: Mirror, target: SwitchTarget): boolean {
  return "projectId" in target
    ? autopilotProjectOn(m.autopilotDisabledProjects, target.projectId)
    : !m.autopilotPausedAgents.includes(target.agentId);
}

/** `m`'s lists with `target` switched `on`. */
function withSwitch(m: Mirror, target: SwitchTarget, on: boolean): Partial<Mirror> {
  if ("agentId" in target) {
    return { autopilotPausedAgents: withMember(m.autopilotPausedAgents, target.agentId, !on) };
  }
  // Unknown opt-outs have nothing to flip from; the host's answer fills them.
  if (m.autopilotDisabledProjects === null) return {};
  return {
    autopilotDisabledProjects: withMember(m.autopilotDisabledProjects, target.projectId, !on),
  };
}

/** Every resync starts a load without waiting for the last, so the newest owns
 *  the mirror (util/newestWins). */
const autopilotLoads = newestWins();

/** Events seen while a load is in flight, replayed over its answer so a snapshot
 *  cannot undo what the host said after reading it — the mirror's (rows and
 *  switches) in arrival order. Null when no load is out. */
let inFlight: { mirror: ((m: Mirror) => Mirror)[]; entries: AutopilotLogEntry[] } | null = null;

/** Switch writes in click order: only the newest one's answer (or failure)
 *  touches the lists, so a slow earlier reply can't undo a later click. */
const switchWrites = newestWins();

export const createAutopilotSlice: SliceCreator<AutopilotSlice> = (set, get) => {
  const writeSwitch = async (target: SwitchTarget, enabled: boolean) => {
    const gated = gateReason(activeEnvironment(), "autopilot");
    if (gated) {
      get().setLastError(gated);
      return;
    }
    const claim = switchWrites.claim();
    const wasOn = switchOn(get(), target);
    // Optimistic, so the switch answers at once; the host's reply is the truth.
    set((s) => withSwitch(s, target, enabled));
    try {
      const snapshot = await forActiveEnvironment(() => api.setAutopilot(target, enabled));
      if (!snapshot || !claim.current()) return;
      // Only the lists: the rows it changed arrive as `autopilot:state`, and a
      // row in this reply may be older than an event that beat it here.
      set({
        autopilotDisabledProjects: snapshot.disabled_projects,
        autopilotPausedAgents: snapshot.paused_agents,
      });
    } catch (e) {
      if (claim.current()) set((s) => withSwitch(s, target, wasOn));
      get().setLastError(String(e));
    }
  };

  return {
    autopilot: {},
    autopilotLog: {},
    autopilotDisabledProjects: null,
    autopilotPausedAgents: [],

    loadAutopilot: async () => {
      const env = activeEnvironment();
      if (env.kind === "remote" && !hostSupports(env.protocol, "autopilot_state")) return;
      const claim = autopilotLoads.claim();
      const buffer: NonNullable<typeof inFlight> = { mirror: [], entries: [] };
      inFlight = buffer;
      try {
        const answer = await forActiveEnvironment(() =>
          Promise.all([api.getAutopilotState(), api.getAutopilotLog()]),
        );
        if (!answer || !claim.current()) return;
        const [snapshot, entries] = answer;
        // Each fold prepends, so fold oldest first: the host's history (newest
        // first) reversed, then what arrived while it was being read.
        const log = [...entries]
          .reverse()
          .concat(buffer.entries)
          .reduce(foldEntry, {} as Log);
        const mirror = buffer.mirror.reduce((m, fold) => fold(m), mirrorOf(snapshot));
        set({ ...mirror, autopilotLog: log });
      } catch {
        // Best effort, like the other resync reads: the events keep it current,
        // and opt-outs never loaded stay unknown.
      } finally {
        if (inFlight === buffer) inFlight = null;
      }
    },

    applyAutopilotState: (row) => {
      inFlight?.mirror.push((m) => foldRow(m, row));
      set((s) => foldRow(s, row));
    },

    applyAutopilotSwitches: (switches) => {
      inFlight?.mirror.push((m) => foldSwitches(m, switches));
      set((s) => foldSwitches(s, switches));
    },

    applyAutopilotEvent: (entry) => {
      inFlight?.entries.push(entry);
      set((s) => ({ autopilotLog: foldEntry(s.autopilotLog, entry) }));
    },

    setProjectAutopilot: (projectId, enabled) => writeSwitch({ projectId }, enabled),
    setAgentAutopilot: (agentId, enabled) => writeSwitch({ agentId }, enabled),
  };
};
