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
  PrChecks,
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

/** `set` with `state` written into its entry, newest number first. The entry
 *  keeps its last-known checks unless `checks` brings new ones (null or absent
 *  is "nothing to say this round"). */
export function upsertState(
  set: PrSetEntry[] | undefined,
  state: PrState,
  checks?: PrChecks | null,
): PrSetEntry[] {
  const prev = set?.find((p) => p.state.number === state.number);
  const rest = (set ?? []).filter((p) => p.state.number !== state.number);
  return [...rest, { state, checks: checks ?? prev?.checks ?? null }].sort(
    (a, b) => b.state.number - a.state.number,
  );
}

/** Whether a PR event is about the checkout's focused PR — the one the legacy
 *  single-PR maps (and the phone) hold. An explicit `focused` flag decides
 *  (`pr:state_changed`). Otherwise the event's PR `number` is compared with the
 *  focused one's: no `number` is a host from before PR sets, which only ever
 *  reported the focused PR, and with no focused PR known the event is taken as
 *  its, as it always was. */
export function isFocusedEvent(
  focused: boolean | undefined,
  number: number | undefined,
  focusedNumber: number | null | undefined,
): boolean {
  if (focused !== undefined) return focused;
  return number === undefined || focusedNumber == null || number === focusedNumber;
}

type StateSlices = Pick<GitSlice, "prStates" | "prSets" | "prChecks" | "prComments">;

export function applyPrStateChanged(s: StateSlices, e: PrStateChangedEvent): Partial<StateSlices> {
  const key = keyOf(e);
  const out: Partial<StateSlices> = {};
  // A null state ("no bound PR") names no PR, so there is no set entry to touch.
  const state = e.state;
  if (state) {
    stampPrWrite("prSets", key);
    // Absent `focused` is a host from before PR sets: it binds one PR per
    // checkout, so its latest report is the whole set (that entry keeping its
    // checks) — upserting would keep a PR it has since replaced as a phantom
    // sibling.
    const prev = s.prSets[key];
    const base =
      e.focused === undefined ? prev?.filter((p) => p.state.number === state.number) : prev;
    out.prSets = { ...s.prSets, [key]: upsertState(base, state) };
  }
  if (isFocusedEvent(e.focused, undefined, undefined)) {
    stampPrWrite("prStates", key);
    out.prStates = { ...s.prStates, [key]: state };
    // The focus moved: the user switched PRs, or the host refocused on one
    // just opened. The legacy checks and threads are the previous PR's: take
    // the set's checks for the new one (none known → none) and drop the
    // threads, so the focused one-shot reads (`focusedPrReads`) fetch its own.
    // A cold checkout (nothing cached yet) has nothing stale to replace.
    const before = s.prStates[key];
    if (state && before !== undefined && before?.number !== state.number) {
      stampPrWrite("prChecks", key);
      stampPrWrite("prComments", key);
      const checks = out.prSets?.[key]?.find((p) => p.state.number === state.number)?.checks;
      const { [key]: _checks, ...prChecks } = s.prChecks;
      const { [key]: _threads, ...prComments } = s.prComments;
      out.prChecks = checks ? { ...prChecks, [key]: checks } : prChecks;
      out.prComments = prComments;
    }
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
  if (isFocusedEvent(undefined, e.number, s.prStates[key]?.number)) {
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
  // Threads are kept for the focused PR alone.
  if (!isFocusedEvent(undefined, e.number, s.prStates[key]?.number)) return {};
  stampPrWrite("prComments", key);
  return { prComments: { ...s.prComments, [key]: e.comments } };
}
