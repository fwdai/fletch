// The host PR watcher's events folded into the PR slices. Each writes one
// checkout's key — `checkoutKey(agent_id, subdir)`, the same key the seed
// (`loadAllPrStatus`) writes — and stamps it, so a seed or one-shot read
// already in flight, which observed the PR before this change, can't land
// afterwards and roll it back (a merged badge flipping back to open).
//
// A checkout holds a set of PRs (`prSets`); the legacy single-PR maps
// (`prStates` / `prChecks` / `prComments`) hold its focused one. The three
// legacy events are only ever about the focused PR, so they write the legacy
// maps unconditionally and upsert the focused entry into the set;
// `pr:set_entry_changed` reports the rest of the set and writes the set alone.
// Only the slices actually written are stamped.

import type { Workspace } from "@/api/types/agent";
import type {
  PrChecks,
  PrChecksChangedEvent,
  PrSetEntry,
  PrSetEntryChangedEvent,
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

type StateSlices = Pick<GitSlice, "prStates" | "prSets" | "prChecks" | "prComments"> & {
  /** The workspace record, when the caller holds it (the store does; the
   *  reducer tests may not). */
  workspace?: Workspace | null;
};

/** The binding a `pr:state_changed` event announces, mirrored onto the
 *  checkout's record as the host's `bind_pr` copies the PR's snapshot onto the
 *  worktree row — so a reader that keys on `repo.pr_number` (`checkoutBase`)
 *  sees the focus the moment it moves, not at the next workspace refresh.
 *  Undefined when the record isn't held, doesn't know the checkout, or already
 *  says this. */
function bindOnRecord(
  workspace: Workspace | null | undefined,
  e: PrStateChangedEvent,
): Workspace | undefined {
  if (!workspace) return undefined;
  const agent = workspace.agents.find((a) => a.id === e.agent_id);
  if (!agent) return undefined;
  const i = e.subdir ? agent.repos.findIndex((r) => r.subdir === e.subdir) : 0;
  const repo = agent.repos[i];
  if (!repo) return undefined;
  const bound = {
    pr_number: e.state?.number ?? null,
    pr_url: e.state?.url ?? null,
    pr_title: e.state?.title ?? null,
    pr_state: e.state?.state ?? null,
  };
  if (
    repo.pr_number === bound.pr_number &&
    repo.pr_url === bound.pr_url &&
    repo.pr_title === bound.pr_title &&
    repo.pr_state === bound.pr_state
  ) {
    return undefined;
  }
  const repos = agent.repos.map((r, j) => (j === i ? { ...r, ...bound } : r));
  return {
    ...workspace,
    agents: workspace.agents.map((a) => (a === agent ? { ...a, repos } : a)),
  };
}

/** The checkout's focused PR, as the host found it. `wholeSet` is a host from
 *  before PR sets, which binds one PR per checkout: its PR is the whole set,
 *  so one it has since replaced must not linger as a sibling. */
export function applyPrStateChanged(
  s: StateSlices,
  e: PrStateChangedEvent,
  wholeSet = false,
): Partial<StateSlices> {
  const key = keyOf(e);
  const state = e.state;
  stampPrWrite("prStates", key);
  const out: Partial<StateSlices> = { prStates: { ...s.prStates, [key]: state } };
  const workspace = bindOnRecord(s.workspace, e);
  if (workspace) out.workspace = workspace;
  if (state) {
    stampPrWrite("prSets", key);
    const prev = s.prSets[key];
    const base = wholeSet ? prev?.filter((p) => p.state.number === state.number) : prev;
    out.prSets = { ...s.prSets, [key]: upsertState(base, state) };
  }
  // The event is the focused PR by definition, so a number other than the one
  // held is the focus moving: the user switched PRs, or the host refocused on
  // one just opened. The legacy checks and threads are the previous PR's: take
  // the set's checks for the new one (none known → none) and drop the threads,
  // so the focused one-shot reads (`focusedPrReads`) fetch its own. A cold
  // checkout (nothing cached yet) has nothing stale to replace.
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
  return out;
}

type ChecksSlices = Pick<GitSlice, "prChecks" | "prSets">;

/** The focused PR's checks: the legacy map, and its entry in the set. */
export function applyPrChecksChanged(
  s: ChecksSlices,
  e: PrChecksChangedEvent,
): Partial<ChecksSlices> {
  const key = keyOf(e);
  stampPrWrite("prChecks", key);
  const out: Partial<ChecksSlices> = { prChecks: { ...s.prChecks, [key]: e.checks } };
  const set = s.prSets[key];
  if (set?.some((p) => p.state.number === e.number)) {
    stampPrWrite("prSets", key);
    out.prSets = {
      ...s.prSets,
      [key]: set.map((p) => (p.state.number === e.number ? { ...p, checks: e.checks } : p)),
    };
  }
  return out;
}

/** The focused PR's threads — the only PR whose threads the host reads. */
export function applyPrThreadsChanged(
  s: Pick<GitSlice, "prComments">,
  e: PrThreadsChangedEvent,
): Pick<GitSlice, "prComments"> {
  const key = keyOf(e);
  stampPrWrite("prComments", key);
  return { prComments: { ...s.prComments, [key]: e.comments } };
}

/** Another PR of the checkout's set: its entry, never the legacy maps. */
export function applyPrSetEntryChanged(
  s: Pick<GitSlice, "prSets">,
  e: PrSetEntryChangedEvent,
): Pick<GitSlice, "prSets"> {
  const key = keyOf(e);
  stampPrWrite("prSets", key);
  return {
    prSets: { ...s.prSets, [key]: upsertState(s.prSets[key], e.entry.state, e.entry.checks) },
  };
}
