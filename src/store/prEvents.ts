// The host PR watcher's three events folded into the PR slices. Each writes
// one checkout's key — `checkoutKey(agent_id, subdir)`, the same key the seed
// (`loadAllPrStatus`) writes — and stamps it, so a seed or one-shot read
// already in flight, which observed the PR before this change, can't land
// afterwards and roll it back (a merged badge flipping back to open).
//
// A checkout holds a set of PRs (`prSets`) and the watcher reports each of
// them; the legacy single-PR maps (`prStates` / `prChecks` / `prComments`) hold
// only the focused one, so an event about another PR of the set must not land
// there. Only the slices actually written are stamped.

import type {
  PrChecksChangedEvent,
  PrSetEntry,
  PrState,
  PrStateChangedEvent,
  PrThreadsChangedEvent,
} from "@/api/types/pr";
import { checkoutKey, type GitSlice } from "./git";
import { stampPrWrite } from "./prWriteOrder";

const keyOf = (e: { agent_id: string; subdir?: string | null }): string =>
  checkoutKey(e.agent_id, e.subdir ?? undefined);

/** `set` with `state` written into its entry (keeping that entry's checks),
 *  newest number first. */
function upsertState(set: PrSetEntry[] | undefined, state: PrState): PrSetEntry[] {
  const prev = set?.find((p) => p.state.number === state.number);
  const rest = (set ?? []).filter((p) => p.state.number !== state.number);
  return [...rest, { state, checks: prev?.checks ?? null }].sort(
    (a, b) => b.state.number - a.state.number,
  );
}

type StateSlices = Pick<GitSlice, "prStates" | "prSets">;

export function applyPrStateChanged(s: StateSlices, e: PrStateChangedEvent): Partial<StateSlices> {
  const key = keyOf(e);
  const out: Partial<StateSlices> = {};
  // A null state ("no bound PR") names no PR, so there is no set entry to touch.
  if (e.state) {
    stampPrWrite("prSets", key);
    out.prSets = { ...s.prSets, [key]: upsertState(s.prSets[key], e.state) };
  }
  // Absent `focused` is a host from before PR sets, which only ever reported
  // the focused PR.
  if (e.focused !== false) {
    stampPrWrite("prStates", key);
    out.prStates = { ...s.prStates, [key]: e.state };
  }
  return out;
}

type ChecksSlices = Pick<GitSlice, "prStates" | "prChecks" | "prSets">;

export function applyPrChecksChanged(
  s: ChecksSlices,
  e: PrChecksChangedEvent,
): Partial<Pick<GitSlice, "prChecks" | "prSets">> {
  const key = keyOf(e);
  const out: Partial<Pick<GitSlice, "prChecks" | "prSets">> = {};
  const set = s.prSets[key];
  if (set?.some((p) => p.state.number === e.number)) {
    stampPrWrite("prSets", key);
    out.prSets = {
      ...s.prSets,
      [key]: set.map((p) => (p.state.number === e.number ? { ...p, checks: e.checks } : p)),
    };
  }
  // With no focused PR known the checks are taken as its, as they always were.
  const focused = s.prStates[key];
  if (focused == null || focused.number === e.number) {
    stampPrWrite("prChecks", key);
    out.prChecks = { ...s.prChecks, [key]: e.checks };
  }
  return out;
}

export function applyPrThreadsChanged(
  s: Pick<GitSlice, "prStates" | "prComments">,
  e: PrThreadsChangedEvent,
): Partial<Pick<GitSlice, "prComments">> {
  const key = keyOf(e);
  // Threads are kept for the focused PR alone. No `number` is a host from
  // before PR sets, which only reported the focused PR; with no focused PR
  // known they are taken as its, like checks.
  const focused = s.prStates[key];
  if (e.number !== undefined && focused != null && e.number !== focused.number) return {};
  stampPrWrite("prComments", key);
  return { prComments: { ...s.prComments, [key]: e.comments } };
}
