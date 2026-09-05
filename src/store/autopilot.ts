// Autopilot enrollment + cycle bookkeeping. The policy is in `@/autopilot`
// (pure); this slice is the state it reads and the transitions the driver
// (`autopilotSync`) applies to it.
//
// Autopilot is ON by default, per project: the switch lives in `project_settings`
// (`AUTOPILOT_ENABLED_KEY`) and is mirrored here as the list of projects that
// turned it off. The driver enrolls every live checkout of an enabled project on
// its own; the per-checkout entry below is runtime bookkeeping, not consent.
//
// The one per-checkout intent that persists is `paused`: a PR the user parked
// should stay parked across a reload rather than quietly resuming. Cycles are NOT
// persisted — an in-flight cycle's agent turn doesn't survive a restart either,
// so resuming one would be judging a turn that never finished. A restart drops
// back to "enrolled, no cycle", and the next tick re-derives from the live world.

import type { VerificationReport } from "@/api";
import type { AutopilotState, Cycle, CyclePhase, StuckReason } from "@/autopilot";
import { newEnrollment } from "@/autopilot";
import type { DelegationKind } from "@/delegation";
import {
  AUTOPILOT_ENABLED_KEY,
  deleteProjectSetting,
  loadAutopilotDisabledProjects,
  setProjectSetting,
} from "@/storage/projectSettings";
import { setSetting } from "@/storage/settings";
import { createKeyedQueue } from "@/util/keyedQueue";
import type { SliceCreator } from "./types";

/** Settings key holding the persisted per-checkout intent (paused checkouts). */
export const AUTOPILOT_SETTING = "autopilotEnrollment";

/** The persisted shape: only the user's intent, never in-flight machinery. */
interface PersistedEnrollment {
  paused: boolean;
}

/** The one answer to "is autopilot on for this project?", shared by the driver,
 *  the Git panel chip and the settings toggle so they can never disagree.
 *
 *  `disabled === null` means the opt-outs are unknown (not loaded, or the load
 *  failed) and the answer is NO for every project: a loop that is on by default
 *  must fail closed when it cannot tell who opted out. */
export function autopilotProjectOn(disabled: readonly string[] | null, projectId: string): boolean {
  return disabled !== null && !disabled.includes(projectId);
}

/** Project-switch writes, serialized per project. Two quick clicks on one toggle
 *  are two writes to the same row; without ordering, the slower first write can
 *  land after the faster second and persist the opposite of the last click. */
const switchWrites = createKeyedQueue();

/** The newest switch request per project. A failed write may only roll the
 *  store back if it is still the latest request for its project — a stale
 *  rollback would otherwise replace a later choice that succeeded. */
const switchSeq = new Map<string, number>();

/** Projects whose row is KNOWN to say off — what the table holds as last
 *  confirmed, seeded by a load and advanced only by a write that succeeded.
 *
 *  This, not the inverse of the failed click, is what a rollback restores. The
 *  inverse would assume the click before it persisted; if two clicks in a row
 *  fail (off, then on), the inverse of "on" would show off while the row still
 *  holds the default — and autopilot would come back on at the next launch
 *  behind a switch that says otherwise. */
const durableOff = new Set<string>();

/** Generation of the opt-out state. Bumped by every load START and every
 *  switch click; a load applies its snapshot only if the generation is still
 *  the one it began in. That drops a load that a later load (Retry during a slow
 *  startup load) or a click (an opt-out persisted while a load was in flight)
 *  has overtaken — either would otherwise replace the newer truth with an older
 *  snapshot, in both the store and `durableOff`. */
let optOutsGen = 0;

export interface AutopilotSlice {
  /** Per-checkout autopilot state, keyed by `checkoutKey`. Absent = the driver
   *  hasn't ticked this checkout yet (or its project has autopilot off). */
  autopilot: Record<string, AutopilotState>;
  /** Projects whose autopilot switch is off (`project_id`s). Every other project
   *  is on — that is the default. Hydrated from `project_settings` at launch and
   *  kept in sync by `setProjectAutopilot`.
   *
   *  `null` until that load succeeds. The driver treats null as "run nothing":
   *  a loop that is on by default must fail CLOSED when it cannot tell which
   *  projects opted out, or a startup hiccup would spend agent turns on exactly
   *  the projects the user switched off. */
  autopilotDisabledProjects: string[] | null;
  /** Local verification autopilot ran to judge the CURRENT cycle, keyed by
   *  `checkoutKey`.
   *
   *  Deliberately not the existing `verificationReports`: that map is keyed by
   *  agent (so a secondary checkout's report would overwrite the primary's) and
   *  is fed by the opt-in turn-end hook, whose latest entry may be evidence from
   *  an unrelated user turn. Cleared when a cycle opens, so a cycle can only ever
   *  be judged by a report produced for it. */
  autopilotVerdicts: Record<string, VerificationReport>;

  /** Load (or reload) the opt-out list from `project_settings`. Called at
   *  launch by `hydrateSettings`, and again by the settings section's retry when
   *  the launch load failed. Leaves the list untouched on failure, so a failed
   *  reload can never turn "unknown" into "everything on". */
  loadAutopilotProjects: () => Promise<void>;
  /** Flip a project's autopilot switch and persist it to `project_settings`.
   *  Optimistic, serialized per project, and rolled back if the write fails
   *  while it is still the latest request. A no-op while the opt-outs are
   *  unknown: there is nothing sound to flip from. */
  setProjectAutopilot: (projectId: string, enabled: boolean) => void;

  /** Start tracking a checkout (the driver does this for every live checkout of
   *  an enabled project; the chip does it when the user acts before the first
   *  tick). */
  enrollAutopilot: (key: string) => void;
  /** Forget a checkout's state and history — used when its agent is gone. */
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

/** Parse the persisted per-checkout intent at launch (mirrors
 *  `parseReviewDismissed` in eventListeners). Rebuilds from a fresh enrollment so
 *  a hand-edited or corrupt row can never inject a cycle, a spent budget, or a
 *  `stuck` the user never saw. An unparseable value yields nothing, which is
 *  safe: the driver re-enrolls live checkouts on its first tick, and the only
 *  thing lost is a paused flag. */
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

/** Persist just the user's intent: which checkouts are paused. Enrollment itself
 *  is not worth a row — the driver re-derives it from live agents on every tick,
 *  and writing every checkout ever seen would grow the row without bound. */
function persist(map: Record<string, AutopilotState>) {
  const out: Record<string, PersistedEnrollment> = {};
  for (const [key, s] of Object.entries(map)) {
    if (s.enrolled && s.paused) out[key] = { paused: true };
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

export const createAutopilotSlice: SliceCreator<AutopilotSlice> = (set, get) => ({
  autopilot: {},
  autopilotVerdicts: {},
  autopilotDisabledProjects: null,

  loadAutopilotProjects: async () => {
    const gen = ++optOutsGen;
    try {
      const disabled = await loadAutopilotDisabledProjects();
      // Overtaken while in flight — this snapshot is already stale. Whatever
      // overtook it (a newer load, a click) owns the state now.
      if (gen !== optOutsGen) return;
      // A load is the truth as of now: it resets what we believe the table holds.
      durableOff.clear();
      for (const id of disabled) durableOff.add(id);
      set({ autopilotDisabledProjects: disabled });
    } catch (e) {
      // Stay (or become) unknown rather than defaulting to "all on" — see
      // `autopilotProjectOn`. Loud, since it silences a whole feature.
      console.error("load autopilot opt-outs failed — autopilot stays off until it loads", e);
    }
  },

  setProjectAutopilot: (projectId, enabled) => {
    // Unknown opt-outs: refuse. Applying a click on top of null would invent an
    // empty list and switch every project on — the exact failure the null exists
    // to prevent. The section disables its toggle for the same reason; this is
    // the store making sure no other caller can do it either.
    if (get().autopilotDisabledProjects === null) return;
    // A click is newer than any load still in flight; that load must not land
    // on top of it (see `optOutsGen`).
    optOutsGen++;

    const apply = (on: boolean) =>
      set((s) => {
        // Guarded above, but a reload could race in between; never fabricate.
        if (s.autopilotDisabledProjects === null) return s;
        const rest = s.autopilotDisabledProjects.filter((id) => id !== projectId);
        return { autopilotDisabledProjects: on ? rest : [...rest, projectId] };
      });
    const seq = (switchSeq.get(projectId) ?? 0) + 1;
    switchSeq.set(projectId, seq);

    // Optimistic, so the toggle answers immediately — but the durable row is the
    // truth. Writes for one project run in click order (`switchWrites`), so the
    // last click is what the row ends up saying. A success advances what we know
    // the row holds; a failure rolls the store back to exactly that — and only
    // if no later click has superseded it.
    apply(enabled);
    switchWrites
      .run(projectId, () =>
        // On is the default, so "on" means no row at all.
        enabled
          ? deleteProjectSetting(projectId, AUTOPILOT_ENABLED_KEY)
          : setProjectSetting(projectId, AUTOPILOT_ENABLED_KEY, "0"),
      )
      .then(
        () => {
          if (enabled) durableOff.delete(projectId);
          else durableOff.add(projectId);
        },
        (e) => {
          console.error("save autopilot.enabled failed", e);
          if (switchSeq.get(projectId) === seq) apply(!durableOff.has(projectId));
        },
      );
  },

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
