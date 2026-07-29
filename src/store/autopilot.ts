// Autopilot enrollment + cycle bookkeeping. The policy is in `@/autopilot`
// (pure); this slice is the state it reads and the transitions the driver
// (`autopilotSync`) applies to it.
//
// Enrollment is persisted, deliberately: a loop the user switched on should
// survive a reload rather than quietly stopping. Cycles are NOT persisted —
// an in-flight cycle's agent turn doesn't survive a restart either, so resuming
// one would be judging a turn that never finished. A restart drops back to
// "enrolled, no cycle", and the next tick re-derives from the live world.

import type { VerificationReport } from "@/api";
import type { AutopilotState, Cycle, CyclePhase, StuckReason } from "@/autopilot";
import { newEnrollment } from "@/autopilot";
import type { DelegationKind } from "@/delegation";
import { setSetting } from "@/storage/settings";
import type { SliceCreator } from "./types";

/** Settings key holding the persisted enrollment set. */
export const AUTOPILOT_SETTING = "autopilotEnrollment";

/** The persisted shape: only the user's intent, never in-flight machinery. */
interface PersistedEnrollment {
  paused: boolean;
}

export interface AutopilotSlice {
  /** Per-checkout autopilot state, keyed by `checkoutKey`. Absent = never
   *  enrolled, which is the default for every checkout. */
  autopilot: Record<string, AutopilotState>;
  /** Local verification autopilot ran to judge the CURRENT cycle, keyed by
   *  `checkoutKey`.
   *
   *  Deliberately not the existing `verificationReports`: that map is keyed by
   *  agent (so a secondary checkout's report would overwrite the primary's) and
   *  is fed by the opt-in turn-end hook, whose latest entry may be evidence from
   *  an unrelated user turn. Cleared when a cycle opens, so a cycle can only ever
   *  be judged by a report produced for it. */
  autopilotVerdicts: Record<string, VerificationReport>;

  /** Turn autopilot on for a checkout. */
  enrollAutopilot: (key: string) => void;
  /** Turn it off entirely and forget the checkout's history. */
  unenrollAutopilot: (key: string) => void;
  /** Hold without losing enrollment; resuming is one click. */
  pauseAutopilot: (key: string) => void;
  /** Resume, clearing any `stuck` state and its spent budget — an explicit
   *  human "try again", which is the only thing that may clear it. */
  resumeAutopilot: (key: string) => void;

  // ── driver transitions (called by autopilotSync, not by the UI) ──
  /** Open a cycle for a dispatched rung. */
  openAutopilotCycle: (key: string, rung: DelegationKind, signature: string) => void;
  /** Move the in-flight cycle to a new phase, restamping its clock. */
  advanceAutopilotCycle: (key: string, phase: CyclePhase, now: number) => void;
  /** The cycle worked: clear it and give the rung its budget back. */
  settleAutopilotCycle: (key: string, rung: DelegationKind) => void;
  /** The cycle failed with budget left: clear it, count the attempt, and record
   *  a barren signature when the world didn't move. */
  retryAutopilotCycle: (key: string, rung: DelegationKind, barren: string | null) => void;
  /** Hand the checkout back to the user. `blockers` is the situation that
   *  stopped it, so `autopilotStep` can tell when that situation has passed. */
  markAutopilotStuck: (
    key: string,
    reason: StuckReason,
    rung: DelegationKind | null,
    now: number,
    blockers: string,
  ) => void;
  /** The situation that stopped it has changed — start looking again.
   *
   *  Distinct from `resumeAutopilot`: this is autopilot noticing the world moved,
   *  not the user insisting. It grants a fresh budget (a new situation deserves
   *  its own attempts) but KEEPS `barren`, so a world autopilot has already
   *  proven it cannot change stays refused even if the checkout oscillates back
   *  to it. A human saying "try again" clears barren too. */
  reviveAutopilot: (key: string) => void;
  /** Store the local verification that will judge this cycle. */
  recordAutopilotVerdict: (key: string, report: VerificationReport) => void;
}

/** Parse the persisted enrollment set at launch (mirrors `parseReviewDismissed`
 *  in eventListeners). Rebuilds from a fresh enrollment so a hand-edited or
 *  corrupt row can never inject a cycle, a spent budget, or a `stuck` the user
 *  never saw — and an unparseable value yields nothing enrolled, which is the
 *  right way for a loop that spends agent turns to fail. */
export function parseAutopilotEnrollment(raw: string | undefined): Record<string, AutopilotState> {
  if (!raw) return {};
  try {
    const parsed: Record<string, PersistedEnrollment> = JSON.parse(raw);
    const out: Record<string, AutopilotState> = {};
    for (const [key, value] of Object.entries(parsed)) {
      out[key] = { ...newEnrollment(), paused: Boolean(value?.paused) };
    }
    return out;
  } catch {
    return {};
  }
}

/** Persist just the enrollment intent for every enrolled checkout. */
function persist(map: Record<string, AutopilotState>) {
  const out: Record<string, PersistedEnrollment> = {};
  for (const [key, s] of Object.entries(map)) {
    if (s.enrolled) out[key] = { paused: s.paused };
  }
  void setSetting(AUTOPILOT_SETTING, out);
}

/** Update one checkout's state and persist, skipping absent entries. */
const patch = (
  set: Parameters<SliceCreator<AutopilotSlice>>[0],
  key: string,
  fn: (s: AutopilotState) => AutopilotState,
  { save = false }: { save?: boolean } = {},
) => {
  set((store) => {
    const current = store.autopilot[key];
    if (!current) return store;
    const autopilot = { ...store.autopilot, [key]: fn(current) };
    if (save) persist(autopilot);
    return { autopilot };
  });
};

/** Drop a checkout's stale verdict — called whenever a cycle opens or ends, so a
 *  report can never outlive the cycle it was produced for. */
const clearVerdict = (verdicts: Record<string, VerificationReport>, key: string) => {
  const { [key]: _dropped, ...rest } = verdicts;
  return rest;
};

export const createAutopilotSlice: SliceCreator<AutopilotSlice> = (set) => ({
  autopilot: {},
  autopilotVerdicts: {},

  enrollAutopilot: (key) => {
    set((s) => {
      const autopilot = { ...s.autopilot, [key]: newEnrollment() };
      persist(autopilot);
      return { autopilot };
    });
  },

  unenrollAutopilot: (key) => {
    set((s) => {
      const { [key]: _dropped, ...autopilot } = s.autopilot;
      persist(autopilot);
      return { autopilot };
    });
  },

  pauseAutopilot: (key) =>
    // Drop any in-flight cycle: the agent's turn continues (we don't interrupt
    // it), but autopilot stops judging it, so resuming re-derives from the world
    // rather than from a verdict nobody was waiting for.
    patch(set, key, (s) => ({ ...s, paused: true, cycle: null }), { save: true }),

  resumeAutopilot: (key) =>
    // Clearing `stuck` AND the spent attempts is the point: the human has looked
    // and wants another go. Barren signatures are cleared too — the world may
    // have changed under them.
    patch(set, key, (s) => ({ ...s, paused: false, stuck: null, attempts: {}, barren: [] }), {
      save: true,
    }),

  openAutopilotCycle: (key, rung, signature) => {
    patch(set, key, (s) => ({
      ...s,
      // `phaseSince` is stamped when the phase that needs a clock
      // (`awaiting-evidence`) is entered, so `working` carries the dispatch time
      // only for display.
      cycle: {
        rung,
        attempt: (s.attempts[rung] ?? 0) + 1,
        signature,
        phase: "working",
        phaseSince: 0,
      } satisfies Cycle,
    }));
    // The previous cycle's verdict must not be read as this one's.
    set((s) => ({ autopilotVerdicts: clearVerdict(s.autopilotVerdicts, key) }));
  },

  advanceAutopilotCycle: (key, phase, now) =>
    patch(set, key, (s) => (s.cycle ? { ...s, cycle: { ...s.cycle, phase, phaseSince: now } } : s)),

  settleAutopilotCycle: (key, rung) =>
    patch(set, key, (s) => ({
      ...s,
      cycle: null,
      // Success earns a fresh budget: a long-lived PR that autopilot keeps
      // helping shouldn't hit a lifetime cap, only a non-converging stretch.
      attempts: { ...s.attempts, [rung]: 0 },
    })),

  retryAutopilotCycle: (key, rung, barren) =>
    patch(set, key, (s) => ({
      ...s,
      cycle: null,
      attempts: { ...s.attempts, [rung]: (s.attempts[rung] ?? 0) + 1 },
      barren: barren && !s.barren.includes(barren) ? [...s.barren, barren] : s.barren,
    })),

  markAutopilotStuck: (key, reason, rung, now, blockers) =>
    patch(set, key, (s) => ({ ...s, cycle: null, stuck: { reason, rung, at: now, blockers } })),

  reviveAutopilot: (key) =>
    patch(set, key, (s) => ({ ...s, stuck: null, cycle: null, attempts: {} })),

  recordAutopilotVerdict: (key, report) =>
    set((s) => ({ autopilotVerdicts: { ...s.autopilotVerdicts, [key]: report } })),
});
