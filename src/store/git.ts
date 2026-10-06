import {
  api,
  type DelegationEvent,
  type GitMeta,
  type GitState,
  type PrChecks,
  type PrComments,
  type PrSetEntry,
  type PrState,
  type ShortStats,
  type VerificationReport,
} from "@/api";
import type { GitCommitAction } from "@/components/RightPanel/primaryActions";
import type { Delegation } from "@/delegation";
import { hostSupports } from "@/remote/types";
import {
  DEFAULT_AUTO_ARCHIVE_IDLE_DAYS,
  DEFAULT_PUBLISH_APPROVAL_WAIT,
} from "@/storage/preferences";
import { setSetting } from "@/storage/settings";
import { newestWins } from "@/util/newestWins";
import { activeEnvironment, activeEnvironmentId, forActiveEnvironment } from "./environments";
import { gateReason } from "./gates";
import { upsertState } from "./prEvents";
import { acceptPrWrite, issuePrWrite, stampPrWrite } from "./prWriteOrder";
import type { SliceCreator } from "./types";

export interface GitSlice {
  /** Full git state — branch, ahead/behind, file list, totals. Keyed by
   *  `checkoutKey(agentId, subdir?)` (store/git.ts): the plain agent_id addresses
   *  the primary repo, `agentId::subdir` a secondary repo of a multi-repo
   *  agent. Only populated for the focused agent (by `gitSync`, for every one of
   *  its repos). For sidebar shortstats / right-rail badges of other agents,
   *  read from `gitShortstats` instead. */
  gitStates: Record<string, GitState>;
  /** Config keys Fletch refuses to run git over, by `checkoutKey`, for each
   *  checkout whose last poll came back blocked (`GitState.blocked_config`).
   *  Kept out of `gitStates` on purpose: a blocked reply is a zero-state — no
   *  files, nothing unpushed — which delegation, autopilot and the badges would
   *  read as "committed and pushed". They keep the last real state instead. */
  gitBlocked: Record<string, string[]>;
  /** Compact per-agent shortstats (additions / deletions / file count),
   *  keyed by agent_id. Updated for every live agent on the app-wide 5s
   *  poll — kept in its own map so the focused agent's richer `gitStates`
   *  entry isn't clobbered by a slower bulk reply. */
  gitShortstats: Record<string, ShortStats>;
  /** Advisory per-checkout git metadata (base staleness + changed-file paths),
   *  keyed by `checkoutKey(agentId, subdir?)`. Fed by the app-wide `getAllGitMeta`
   *  poll — separate from `gitShortstats` (badge numbers) so each evolves on its
   *  own cadence. Drives the "base moved" staleness chips and overlap hints. */
  gitMeta: Record<string, GitMeta>;
  /** PR state, keyed by `checkoutKey(agentId, subdir?)` — plain agent_id for the
   *  primary repo, `agentId::subdir` for a secondary repo. Seeded by
   *  `loadAllPrStatus` and the focused checkouts' one-shot `fetchPrLive`, then
   *  kept current by the host watcher's `pr:state_changed`. */
  prStates: Record<string, PrState | null>;
  /** Rich PR merge-gate + checks, keyed by `checkoutKey(agentId, subdir?)`. Absent
   *  key = not yet fetched; `null` = confirmed unavailable (no PR / gh
   *  failure). Seeded like `prStates`, then followed via `pr:checks_changed`. */
  prChecks: Record<string, PrChecks | null>;
  /** Unresolved PR review comments, keyed by `checkoutKey(agentId, subdir?)`.
   *  Absent = not yet fetched; `null` = confirmed unavailable (no PR / gh
   *  failure). Read once per focused checkout (`fetchPrThreads`), then followed
   *  via `pr:threads_changed`. */
  prComments: Record<string, PrComments | null>;
  /** Every PR of the checkout, focused included, newest number first, keyed by
   *  `checkoutKey(agentId, subdir?)`. A checkout holds a set of PRs (sub-agents
   *  each open their own); the legacy three maps above hold the focused PR.
   *  Seeded by `loadAllPrStatus`, then followed via `pr:state_changed` /
   *  `pr:checks_changed`. Threads are only kept for the focused PR. */
  prSets: Record<string, PrSetEntry[]>;
  /** Live host delegations per checkout, keyed by `checkoutKey(agentId,
   *  subdir?)` (absent = none). A MIRROR: the host owns the lifecycle
   *  (`supervisor::delegation`), and this is fed by `delegation:changed` and by
   *  `get_delegations` on bootstrap, reconnect and environment switch, so every
   *  window driving the host shows the same thing at the same moment. */
  delegations: Record<string, Delegation>;
  /** Transient outcome text for a settled delegation, keyed by checkout — the
   *  "Conflicts resolved" / "Agent finished" confirmation the host's `done` /
   *  `abandoned` event carries. Rendered by the matching Git panel section if
   *  one is mounted; self-expires, so an outcome nobody was looking at lapses. */
  delegationNotices: Record<string, string>;
  /** Latest turn-end verification report per agent (keyed by agent_id), from
   *  the opt-in `verify:report` event. Feeds the Mission Control card's tests
   *  chip. Absent = never verified (no chip). */
  verificationReports: Record<string, VerificationReport>;
  /** Sticky changes-state commit mode (Commit / & push / & open PR). Global
   *  across workspaces, persisted in settings until the user picks another. */
  gitCommitAction: GitCommitAction;
  /** Seconds a publish-approval prompt waits before denying; 0 = until
   *  answered. Mirrors the backend-owned `publish_approval_wait` setting. */
  publishApprovalWait: number;
  /** Prefix prepended to every branch an agent creates ("" = none). Mirrors the
   *  backend-owned `git_branch_prefix` setting. */
  branchPrefix: string;
  /** Days an idle, clean, fully pushed workspace waits before the backend's
   *  sweep archives it; 0 = off. Mirrors the backend-owned
   *  `auto_archive_idle_days` setting. Lives here with the other backend-owned
   *  workspace preferences, though it is not a git setting. */
  autoArchiveIdleDays: number;
  /** Open pull requests as drafts. Mirrors the backend-owned `github_draft_prs`
   *  setting. */
  draftPrs: boolean;

  /** Fetch full git state for one agent (used by the focused panel's poll).
   *  `subdir` targets a secondary repo of a multi-repo agent (stored under
   *  `checkoutKey(agentId, subdir)`); omitted = the primary repo, plain key. */
  fetchGitState: (agentId: string, subdir?: string) => Promise<void>;
  /** Fetch compact shortstats for every live agent in one round-trip
   *  (used by the app-wide background poll). */
  fetchAllShortstats: () => Promise<void>;
  /** Fetch advisory git metadata (base staleness + file paths) for every live
   *  checkout in one round-trip (app-wide background poll, local git only). */
  fetchAllGitMeta: () => Promise<void>;
  fetchPrState: (agentId: string, subdir?: string) => Promise<void>;
  /** Seed the fleet's PR state + CI from one `get_all_pr_status` read: every
   *  repo with a known PR across every agent, keyed by `checkoutKey`, so the
   *  sidebar badges are right without opening a panel. A seed, not a poll —
   *  run on GitHub connecting, environment switch / reconnect and window
   *  focus; between those the host watcher's `pr:*` events keep it current.
   *  `reverifyClosed` asks for a live look at closed PRs (they can reopen). */
  loadAllPrStatus: (reverifyClosed?: boolean) => Promise<void>;
  fetchPrChecks: (agentId: string, subdir?: string) => Promise<void>;
  /** PR state + CI in one backend pass over ETag-conditional REST, both from
   *  the same moment so they can't disagree. `gitSync` reads it once for a
   *  focused checkout with nothing cached (or a PR that just changed); the
   *  watcher's events take it from there. */
  fetchPrLive: (agentId: string, subdir?: string) => Promise<void>;
  /** Unresolved review threads (GraphQL — thread resolution has no REST
   *  equivalent). Read once like `fetchPrLive`; `pr:threads_changed` keeps it
   *  current. */
  fetchPrThreads: (agentId: string, subdir?: string) => Promise<void>;
  /** Hand the playbook `action` to the agent through the host's `delegate_git`
   *  (which composes the trigger, holds it while the agent is mid-turn and
   *  watches it to its end). `params` carry only the dynamic context the
   *  playbook can't know; `subdir` targets a secondary checkout. Mirrors the
   *  recorded delegation at once, so a caller re-reading `delegations` this tick
   *  sees it; a refusal lands in `lastError`. */
  delegateAction: (
    agentId: string,
    action: string,
    params?: Record<string, string>,
    subdir?: string,
  ) => Promise<void>;
  /** Fold one `delegation:changed` into the mirror: a live phase replaces the
   *  checkout's entry, `done` / `abandoned` drops it and posts its notice. */
  applyDelegationChange: (e: DelegationEvent) => void;
  /** Replace the mirror with the host's table (`get_delegations`). */
  loadDelegations: () => Promise<void>;
  /** Post a settled delegation's outcome for the panel to show, if mounted. */
  noteDelegationOutcome: (key: string, text: string) => void;
  setGitCommitAction: (action: GitCommitAction) => void;
  setPublishApprovalWait: (secs: number) => Promise<void>;
  setAutoArchiveIdleDays: (days: number) => Promise<void>;
  /** Rejects with the backend's validation message; the store is untouched then. */
  setBranchPrefix: (prefix: string) => Promise<void>;
  setDraftPrs: (enabled: boolean) => Promise<void>;
  /** Resolves to "up-to-date" | "pushed" on success, null on error. */
  pushAgent: (agentId: string, subdir?: string) => Promise<string | null>;
  /** Resolves true on success, false on error. */
  pullAgent: (agentId: string, subdir?: string) => Promise<boolean>;
  /** Resolves true on success, false on error. */
  rebaseAgent: (agentId: string, subdir?: string) => Promise<boolean>;
  commitChanges: (agentId: string, message: string, subdir?: string) => Promise<boolean>;
  /** Commit all changes, push, and open a PR — the "Commit & open PR"
   *  primary CTA wired from the git panel. Returns false on any step
   *  failure so the UI can leave the textarea content in place. */
  commitAndOpenPr: (agentId: string, message: string, subdir?: string) => Promise<boolean>;
  stashChanges: (agentId: string, subdir?: string) => Promise<void>;
  discardChanges: (agentId: string, subdir?: string) => Promise<void>;
  abortMerge: (agentId: string, subdir?: string) => Promise<void>;
  /** Remove the config keys blocking the checkout (`GitState.blocked_config`).
   *  True once they are gone and the panel has re-read the checkout. */
  clearCheckoutConfig: (agentId: string, subdir?: string) => Promise<boolean>;
  deleteBranch: (agentId: string, subdir?: string) => Promise<void>;
  createPr: (
    agentId: string,
    title: string,
    body: string,
    subdir?: string,
  ) => Promise<PrState | null>;
  mergePr: (agentId: string, subdir?: string) => Promise<void>;
  /** Publish a local-only project (no origin) to GitHub, then refresh git
   *  state so the panel switches out of the no-origin affordances. Resolves
   *  the repo web URL on success, null on error. */
  publishAgent: (agentId: string, isPrivate: boolean) => Promise<string | null>;
}

type GitSet = Parameters<SliceCreator<GitSlice>>[0];
type GitGet = Parameters<SliceCreator<GitSlice>>[1];

/** Identity of one checkout: an agent plus which of its repos. The primary repo
 *  (no `subdir`) keeps the plain agent id — the key every existing write path
 *  (background bulk polls, tauri event reducers, sidebar badges) uses — so only
 *  secondary-repo fetches get the suffixed form.
 *
 *  Nothing about this is git-specific; it addresses "which working copy of which
 *  agent", which is the scope every per-repo map (`gitStates`, `prStates`,
 *  `prChecks`, `prComments`, `delegations`) is keyed by. */
export function checkoutKey(agentId: string, subdir?: string): string {
  return subdir ? `${agentId}::${subdir}` : agentId;
}

/** Inverse of [`checkoutKey`]. Splits on the FIRST `::` so a subdir containing
 *  the separator still round-trips — the same prefix assumption `maxBehind` and
 *  `dropScopedEntries` already rely on. */
export function splitCheckoutKey(key: string): { agentId: string; subdir?: string } {
  const at = key.indexOf("::");
  return at === -1 ? { agentId: key } : { agentId: key.slice(0, at), subdir: key.slice(at + 2) };
}

/** Max `behind` across an agent's checkouts (the `agentId` primary key plus
 *  every `agentId::subdir` secondary in `gitMeta`), or null when every base is
 *  unknown or fresh — a stale secondary must surface even when the primary is
 *  current. */
export function maxBehind(meta: Record<string, GitMeta>, agentId: string): number | null {
  const prefix = `${agentId}::`;
  let worst: number | null = null;
  for (const [key, m] of Object.entries(meta)) {
    if (key !== agentId && !key.startsWith(prefix)) continue;
    if (m.behind == null || m.behind <= 0) continue;
    if (worst == null || m.behind > worst) worst = m.behind;
  }
  return worst;
}

// Shared shape for the simple git mutations: run the backend call, refresh git
// state on success, otherwise record the error and report failure.
const runGitMutation = async (
  get: GitGet,
  agentId: string,
  fn: () => Promise<unknown>,
  subdir?: string,
): Promise<boolean> => {
  try {
    await fn();
    await get().fetchGitState(agentId, subdir);
    return true;
  } catch (e) {
    get().setLastError(String(e));
    return false;
  }
};

/** A focused-PR read folded into its checkout's set (`prSets`): the entry
 *  upserted with `state` (and `checks`, when the read resolved them), or null
 *  when it must not land there. A merged/closed PR the set doesn't hold is the
 *  display-only answer `resolve_pr_state` gives an unbound checkout (a recycled
 *  branch's old PR), never a member; an open one it found was adopted, i.e.
 *  bound, so it is. */
function focusedIntoSet(
  set: PrSetEntry[] | undefined,
  state: PrState,
  checks?: PrChecks | null,
): PrSetEntry[] | null {
  if (state.state !== "open" && !set?.some((p) => p.state.number === state.number)) return null;
  return upsertState(set, state, checks);
}

/** `prSets` with `next` written for `mapKey`, ticket permitting — or nothing,
 *  for a `set()` to spread. Accepted inside the write, so a read that has
 *  nothing to fold in doesn't claim the slice. */
function setsWrite(
  s: GitSlice,
  mapKey: string,
  ticket: number,
  next: PrSetEntry[] | null,
): Partial<GitSlice> {
  if (!next || !acceptPrWrite("prSets", mapKey, ticket)) return {};
  return { prSets: { ...s.prSets, [mapKey]: next } };
}

// fetchPrChecks/fetchPrComments are identical except for the slice key and the
// backend call: write the value (including null = "confirmed unavailable") on
// success; on a *first* failure degrade the absent key to null so the panel
// drops the "checking…" placeholder, while a later transient error keeps the
// last good value. `toSet` folds an accepted value into the checkout's PR set.
const fetchPrAux = async <K extends "prChecks" | "prComments">(
  set: GitSet,
  agentId: string,
  key: K,
  fetch: (agentId: string, subdir?: string) => Promise<GitSlice[K][string]>,
  subdir?: string,
  toSet?: (s: GitSlice, mapKey: string, value: GitSlice[K][string]) => PrSetEntry[] | null,
): Promise<void> => {
  const mapKey = checkoutKey(agentId, subdir);
  const ticket = issuePrWrite();
  try {
    const value = await fetch(agentId, subdir);
    if (!acceptPrWrite(key, mapKey, ticket)) return;
    set(
      (s) =>
        ({
          [key]: { ...s[key], [mapKey]: value },
          ...setsWrite(s, mapKey, ticket, toSet?.(s, mapKey, value) ?? null),
        }) as Partial<GitSlice>,
    );
  } catch {
    set((s) =>
      mapKey in s[key] ? {} : ({ [key]: { ...s[key], [mapKey]: null } } as Partial<GitSlice>),
    );
  }
};

// Whether GitHub is usable. The reads below early-return on `false` so that a
// user with no connection generates zero API chatter — the rule lives here, in
// the data layer, rather than being re-checked by each poller. Action-driven
// reads (`fetchPrState`, `fetchPrChecks`) are deliberately not gated: they
// resolve against the persisted snapshot, which is still useful offline.
const githubReady = (get: GitGet) => get().github?.authenticated ?? false;

/** How long a delegation outcome stays posted. Matches the Git panel's own
 *  transient-notice window (`useTransientFeedback`) so the two read alike. */
const DELEGATION_NOTICE_MS = 3500;

/** Live expiry timers for `delegationNotices`, by checkout key. Module scope
 *  because they're side-channel cleanup, not observable state. */
const noticeTimers = new Map<string, ReturnType<typeof setTimeout>>();

/** The mirror entry a host delegation report becomes, or null for one that has
 *  ended (`done` / `abandoned`). */
function mirrorOf(e: DelegationEvent): Delegation | null {
  if (e.phase === "done" || e.phase === "abandoned") return null;
  return {
    kind: e.kind,
    phase: e.phase,
    startedAt: e.started_at,
    ...(e.subdir ? { subdir: e.subdir } : {}),
  };
}

/** The fleet PR seed in flight, and the environment it is reading. */
let allPrStatusLoad: { env: string; done: Promise<void> } | null = null;

/** `next` as a checkout's PR set, each PR keeping its last-known checks where
 *  `next` has nothing to say about them (`checks: null`) — the same rule the
 *  seed applies to `prChecks`. */
function withKnownChecks(prev: PrSetEntry[] | undefined, next: PrSetEntry[]): PrSetEntry[] {
  if (!prev) return next;
  return next.map((p) =>
    p.checks != null
      ? p
      : { ...p, checks: prev.find((q) => q.state.number === p.state.number)?.checks ?? null },
  );
}

/** One `get_all_pr_status` read, merged into `prStates` / `prChecks` /
 *  `prSets`. */
async function readAllPrStatus(set: GitSet, reverifyClosed: boolean): Promise<void> {
  const ticket = issuePrWrite();
  try {
    // The reply is the seed itself: the host's watcher emits only on a change,
    // so nothing else would ever tell this window the current state. Dropped if
    // the user switched environment mid-read (agent ids recur across hosts).
    const map = await forActiveEnvironment(() => api.getAllPrStatus(reverifyClosed));
    if (!map) return;
    set((s) => {
      // Merge, never replace: agents absent from the reply keep whatever the
      // focused-panel / per-trigger paths recorded. `checks` is only written
      // when the read resolved one (open PR, live fetch) — a null there means
      // "nothing to say this round", so the last-known tint survives instead of
      // being wiped by a snapshot-served or merged entry.
      //
      // Ticket-checked per key: this read can be slower than a focused
      // fetchPrLive or land after a watcher event, so it must not roll a key
      // back to what it saw earlier.
      const prStates = { ...s.prStates };
      const prChecks = { ...s.prChecks };
      const prSets = { ...s.prSets };
      for (const [key, entry] of Object.entries(map)) {
        if (acceptPrWrite("prStates", key, ticket)) prStates[key] = entry.state;
        if (entry.checks != null && acceptPrWrite("prChecks", key, ticket)) {
          prChecks[key] = entry.checks;
        }
        // A host from before PR sets reports the focused PR alone, which is
        // its whole set as far as it knows.
        if (acceptPrWrite("prSets", key, ticket)) {
          const next = entry.prs ?? [{ state: entry.state, checks: entry.checks }];
          prSets[key] = withKnownChecks(s.prSets[key], next);
        }
      }
      return { prStates, prChecks, prSets };
    });
  } catch {
    // Non-fatal: the badges keep their last state, the watcher's events keep
    // moving them, and the next resync (focus, reconnect) reads again.
  }
}

/** The `started_at` of the last delegation that ended on each checkout, so the
 *  `delegate_git` reply landing after its own `done` cannot put it back. */
const endedDelegations = new Map<string, number>();

/** Every resync starts a `get_delegations` without waiting for the last, so the
 *  newest owns the mirror (util/newestWins). */
const delegationLoads = newestWins();

/** `delegation:changed` events seen while a `get_delegations` is in flight,
 *  replayed over its answer. Null when no load is outstanding. */
let delegationsInFlight: DelegationEvent[] | null = null;

export const createGitSlice: SliceCreator<GitSlice> = (set, get) => ({
  gitStates: {},
  gitBlocked: {},
  gitShortstats: {},
  gitMeta: {},
  prStates: {},
  prChecks: {},
  prComments: {},
  prSets: {},
  delegations: {},
  delegationNotices: {},
  verificationReports: {},
  gitCommitAction: "agent-commit-pr" as GitCommitAction,
  publishApprovalWait: DEFAULT_PUBLISH_APPROVAL_WAIT,
  autoArchiveIdleDays: DEFAULT_AUTO_ARCHIVE_IDLE_DAYS,
  branchPrefix: "",
  draftPrs: false,

  fetchGitState: async (agentId, subdir) => {
    try {
      // Dropped if the user switched environment mid-poll: agent ids recur
      // across hosts, so a late answer would land on the new host's checkout.
      const state = await forActiveEnvironment(() => api.getGitState(agentId, subdir));
      if (!state) return;
      const key = checkoutKey(agentId, subdir);
      const blocked = state.blocked_config ?? [];
      if (blocked.length > 0) {
        set((s) => ({ gitBlocked: { ...s.gitBlocked, [key]: blocked } }));
        return;
      }
      set((s) => {
        const { [key]: _cleared, ...gitBlocked } = s.gitBlocked;
        return { gitStates: { ...s.gitStates, [key]: state }, gitBlocked };
      });
    } catch {
      // non-fatal — next poll tick will retry
    }
  },

  fetchAllShortstats: async () => {
    try {
      const map = await api.getAllShortstats();
      // Replace wholesale — agents archived/removed between ticks fall
      // out naturally. This map is independent of `gitStates`, so the
      // focused panel's full-state poll can't be clobbered.
      set({ gitShortstats: map });
    } catch {
      // non-fatal — next poll tick will retry
    }
  },

  fetchAllGitMeta: async () => {
    try {
      const map = await api.getAllGitMeta();
      // Replace wholesale (like gitShortstats) — agents removed between ticks
      // fall out naturally, and this map is independent of gitStates so the
      // focused panel's full-state poll is never clobbered.
      set({ gitMeta: map });
    } catch {
      // non-fatal — next poll tick will retry
    }
  },

  fetchPrState: async (agentId, subdir) => {
    const mapKey = checkoutKey(agentId, subdir);
    const ticket = issuePrWrite();
    try {
      const state = await api.getPrState(agentId, subdir);
      if (!acceptPrWrite("prStates", mapKey, ticket)) return;
      // Always write (including null) to distinguish "confirmed: no PR" from
      // "not yet fetched" (absent key). Unlike fetchGitState which guards the
      // write, PR state being null is meaningful.
      // The set's entry for it moves too, so it isn't stale until the next
      // sweep; a confirmed "no PR" names no entry to touch.
      set((s) => ({
        prStates: { ...s.prStates, [mapKey]: state },
        ...setsWrite(s, mapKey, ticket, state && focusedIntoSet(s.prSets[mapKey], state)),
      }));
    } catch {
      // non-fatal
    }
  },

  loadAllPrStatus: (reverifyClosed = false) => {
    if (!githubReady(get)) return Promise.resolve();
    const env = activeEnvironment();
    if (env.kind === "remote" && !hostSupports(env.protocol, "get_all_pr_status")) {
      return Promise.resolve();
    }
    // GitHub connecting, an environment switch and a focus can all ask at once
    // (a switch flips `github` as it re-probes); one read answers them all.
    const envId = activeEnvironmentId();
    if (allPrStatusLoad?.env === envId) return allPrStatusLoad.done;
    const done = readAllPrStatus(set, reverifyClosed).finally(() => {
      if (allPrStatusLoad?.done === done) allPrStatusLoad = null;
    });
    allPrStatusLoad = { env: envId, done };
    return done;
  },

  fetchPrChecks: (agentId, subdir) =>
    fetchPrAux(set, agentId, "prChecks", api.getPrChecks, subdir, (s, mapKey, checks) => {
      // Checks are the focused PR's; with none known (or none resolved) there
      // is no entry to write them to.
      const focused = s.prStates[mapKey];
      return focused && checks ? focusedIntoSet(s.prSets[mapKey], focused, checks) : null;
    }),

  fetchPrLive: async (agentId, subdir) => {
    if (!githubReady(get)) return;
    const mapKey = checkoutKey(agentId, subdir);
    const ticket = issuePrWrite();
    try {
      const live = await api.getPrLive(agentId, subdir);
      // One ticket, checked per slice: the fleet seed and the watcher's events
      // write these same keys, so an older response must not land on top of a
      // newer one (nor on top of the post-merge refresh).
      const takeState = acceptPrWrite("prStates", mapKey, ticket);
      const takeChecks =
        (live?.checks != null || live == null) && acceptPrWrite("prChecks", mapKey, ticket);
      if (!takeState && !takeChecks) return;
      // Writing `null` state is safe here *because* the backend degrades a failed
      // lookup to the last persisted snapshot (see `get_pr_live` /
      // `resolve_pr_state`) rather than reporting nothing. So a null reply means
      // "this repo has no PR", not "the fetch failed" — the distinction the
      // panel depends on, since it treats a present null as authoritative and
      // skips its snapshot fallback.
      //
      // Checks are only written when the read resolved them: a null `checks` on a
      // present PR means the CI reads degraded (or the PR isn't open), and
      // overwriting a good rollup there would blank the pill on a transient
      // error.
      //
      // The set's entry for the PR moves with the state (and its checks, when
      // those were taken too), so it isn't stale until the next sweep.
      set((s) => ({
        prStates: takeState ? { ...s.prStates, [mapKey]: live?.state ?? null } : s.prStates,
        prChecks: takeChecks ? { ...s.prChecks, [mapKey]: live?.checks ?? null } : s.prChecks,
        ...setsWrite(
          s,
          mapKey,
          ticket,
          takeState && live
            ? focusedIntoSet(s.prSets[mapKey], live.state, takeChecks ? live.checks : null)
            : null,
        ),
      }));
    } catch {
      // Mirror fetchPrAux's failure contract: degrade an absent key to null so
      // the "checking…" placeholder clears, but let a later transient error
      // keep the last good value.
      set((s) => ({
        prChecks: mapKey in s.prChecks ? s.prChecks : { ...s.prChecks, [mapKey]: null },
      }));
    }
  },

  fetchPrThreads: async (agentId, subdir) => {
    if (!githubReady(get)) return;
    await fetchPrAux(set, agentId, "prComments", api.getPrThreads, subdir);
  },

  delegateAction: async (agentId, action, params, subdir) => {
    // A host that can't run delegations says so rather than being sent a
    // trigger nothing on it would watch. Never closed locally.
    const gated = gateReason(activeEnvironment(), "delegateGit");
    if (gated) {
      get().setLastError(gated);
      return;
    }
    try {
      const recorded = await forActiveEnvironment(() =>
        api.delegateGit(agentId, action, params, subdir),
      );
      if (!recorded) return;
      // The host's `delegation:changed` says the same and may land either side
      // of this reply. Writing now shows the label without waiting for the
      // event; the one thing it must not do is resurrect a delegation whose end
      // already arrived.
      const key = checkoutKey(recorded.agent_id, recorded.subdir ?? undefined);
      const live = mirrorOf(recorded);
      if (!live || endedDelegations.get(key) === recorded.started_at) return;
      set((s) => ({ delegations: { ...s.delegations, [key]: live } }));
    } catch (e) {
      get().setLastError(String(e));
    }
  },

  applyDelegationChange: (e) => {
    delegationsInFlight?.push(e);
    const key = checkoutKey(e.agent_id, e.subdir ?? undefined);
    const live = mirrorOf(e);
    if (live) {
      set((s) => ({ delegations: { ...s.delegations, [key]: live } }));
      return;
    }
    endedDelegations.set(key, e.started_at);
    set((s) => {
      const { [key]: _ended, ...rest } = s.delegations;
      return { delegations: rest };
    });
    if (e.notice) get().noteDelegationOutcome(key, e.notice);
    // A fresh PR (or branch update) changes the merge gate — refresh now rather
    // than waiting out the slow poll.
    if (e.phase === "done") void get().fetchPrChecks(e.agent_id, e.subdir ?? undefined);
  },

  loadDelegations: async () => {
    const env = activeEnvironment();
    if (env.kind === "remote" && !hostSupports(env.protocol, "get_delegations")) return;
    const claim = delegationLoads.claim();
    const buffer: DelegationEvent[] = [];
    delegationsInFlight = buffer;
    try {
      const rows = await forActiveEnvironment(() => api.getDelegations());
      if (!rows || !claim.current()) return;
      // The table as the host read it, with whatever it said since folded back
      // in — an event that raced the read must not be undone by it.
      const delegations: Record<string, Delegation> = {};
      for (const e of [...rows, ...buffer]) {
        const key = checkoutKey(e.agent_id, e.subdir ?? undefined);
        const live = mirrorOf(e);
        if (live) delegations[key] = live;
        else delete delegations[key];
      }
      set({ delegations });
    } catch {
      // Best effort, like the other resync reads: the events keep it current.
    } finally {
      if (delegationsInFlight === buffer) delegationsInFlight = null;
    }
  },

  noteDelegationOutcome: (key, text) => {
    set((s) => ({ delegationNotices: { ...s.delegationNotices, [key]: text } }));
    // Self-expiring: the notice is a confirmation, not state. Clearing on a timer
    // (rather than on the next render) keeps it visible long enough to read when a
    // panel IS mounted, and harmlessly lapses when none is.
    clearTimeout(noticeTimers.get(key));
    noticeTimers.set(
      key,
      setTimeout(() => {
        noticeTimers.delete(key);
        set((s) => {
          const { [key]: _expired, ...rest } = s.delegationNotices;
          return { delegationNotices: rest };
        });
      }, DELEGATION_NOTICE_MS),
    );
  },

  setGitCommitAction: (action) => {
    set({ gitCommitAction: action });
    void setSetting("gitCommitAction", action);
  },

  setPublishApprovalWait: async (secs) => {
    await api.setPublishApprovalWait(secs);
    set({ publishApprovalWait: secs });
  },

  setAutoArchiveIdleDays: async (days) => {
    await api.setAutoArchiveIdleDays(days);
    set({ autoArchiveIdleDays: days });
  },

  setBranchPrefix: async (prefix) => {
    // The backend trims and validates; store what it actually kept.
    const stored = await api.setBranchPrefix(prefix);
    set({ branchPrefix: stored });
  },

  setDraftPrs: async (enabled) => {
    await api.setDraftPrs(enabled);
    set({ draftPrs: enabled });
  },

  pushAgent: async (agentId, subdir) => {
    try {
      // "up-to-date" | "pushed" — lets the UI confirm the outcome.
      const summary = await api.pushAgent(agentId, subdir);
      await get().fetchGitState(agentId, subdir);
      // pr:state_changed event will update prStates automatically
      return summary;
    } catch (e) {
      set({ lastError: String(e) });
      return null;
    }
  },

  pullAgent: (agentId, subdir) =>
    runGitMutation(get, agentId, () => api.pullAgent(agentId, subdir), subdir),

  rebaseAgent: (agentId, subdir) =>
    runGitMutation(get, agentId, () => api.rebaseAgent(agentId, subdir), subdir),

  commitChanges: (agentId, message, subdir) =>
    runGitMutation(get, agentId, () => api.commitAgent(agentId, message, subdir), subdir),

  commitAndOpenPr: async (agentId, message, subdir) => {
    try {
      await api.commitAgent(agentId, message, subdir);
      await api.pushAgent(agentId, subdir);
      const pr = await api.createPr(agentId, "", "", subdir);
      // Authoritative: we just created this PR. Stamp it so a poll that was
      // already in flight — and saw no PR at all — can't erase the card.
      stampPrWrite("prStates", checkoutKey(agentId, subdir));
      set((s) => ({ prStates: { ...s.prStates, [checkoutKey(agentId, subdir)]: pr } }));
      await get().fetchGitState(agentId, subdir);
      return true;
    } catch (e) {
      set({ lastError: String(e) });
      await get().fetchGitState(agentId, subdir);
      return false;
    }
  },

  stashChanges: async (agentId, subdir) => {
    await runGitMutation(get, agentId, () => api.stashAgent(agentId, subdir), subdir);
  },

  discardChanges: async (agentId, subdir) => {
    await runGitMutation(get, agentId, () => api.discardAgentChanges(agentId, subdir), subdir);
  },

  abortMerge: async (agentId, subdir) => {
    await runGitMutation(get, agentId, () => api.abortMergeAgent(agentId, subdir), subdir);
  },

  clearCheckoutConfig: (agentId, subdir) =>
    runGitMutation(get, agentId, () => api.clearCheckoutConfig(agentId, subdir), subdir),

  deleteBranch: async (agentId, subdir) => {
    await runGitMutation(get, agentId, () => api.deleteBranchAgent(agentId, subdir), subdir);
  },

  createPr: async (agentId, title, body, subdir) => {
    try {
      const pr = await api.createPr(agentId, title, body, subdir);
      // Authoritative (see commitAndOpenPr): outranks any in-flight poll.
      stampPrWrite("prStates", checkoutKey(agentId, subdir));
      set((s) => ({ prStates: { ...s.prStates, [checkoutKey(agentId, subdir)]: pr } }));
      return pr;
    } catch (e) {
      set({ lastError: String(e) });
      return null;
    }
  },

  mergePr: async (agentId, subdir) => {
    try {
      await api.mergePr(agentId, subdir);
      // Refresh immediately: no backend event fires on merge, and the panel
      // should transition to the merged state as soon as GitHub reports it
      // (with --auto + pending checks the PR can legitimately stay open).
      // One read covers state and checks; the merge just changed both, so the
      // conditional GETs come back 200 rather than 304 here.
      await get().fetchPrLive(agentId, subdir);
    } catch (e) {
      set({ lastError: String(e) });
    }
  },

  publishAgent: async (agentId, isPrivate) => {
    try {
      const url = await api.publishAgent(agentId, isPrivate);
      // Origin now exists — refresh so the panel drops the no-origin
      // affordances and shows normal push/PR against the new remote.
      await get().fetchGitState(agentId);
      return url;
    } catch (e) {
      set({ lastError: String(e) });
      return null;
    }
  },
});
