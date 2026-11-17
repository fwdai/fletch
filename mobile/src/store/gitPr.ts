// The host's git and PR reads: each agent's working tree, its PR's state, CI
// and review threads, and the fleet-wide stats and PR sweep behind the agent
// rows. Every PR read is ordered against the host watcher's `pr:*` events
// (@desktop/store/prWriteOrder), whose order the store shares with its event
// handlers (store/events).

import type { GitState, ShortStats } from "@desktop/api/types/git";
import type { PrChecks, PrComments, PrState } from "@desktop/api/types/pr";
import type { PrWriteOrder } from "@desktop/store/prWriteOrder";
import { newestWins } from "@desktop/util/newestWins";
import type { HostLink } from "../host";
import type { HostState } from "./host";

type Set = (partial: Partial<HostState> | ((s: HostState) => Partial<HostState>)) => void;
type Get = () => HostState;

export interface GitPrSlice {
  gitStates: Record<string, GitState | null>;
  /** Uncommitted working-tree stats for the whole fleet, from the app-wide
   *  poll — the same numbers the desktop sidebar shows. */
  shortstats: Record<string, ShortStats>;
  prStates: Record<string, PrState | null>;
  prChecks: Record<string, PrChecks | null>;
  /** Unresolved review threads per agent: one `get_pr_threads` when the Ship
   *  tab opens with none cached, then `pr:threads_changed`. Never set on a host
   *  without the op, so a missing key means "unknown", not "none". Emptied on
   *  every handshake — an event missed while the socket was down has no other
   *  way back — so an open tab reads again. */
  prComments: Record<string, PrComments | null>;
  loadGit(agentId: string): Promise<void>;
  /** Read the agent's working-tree state alone — the local git half of
   *  `loadGit`, cheap enough to poll while the Agent screen shows it (see
   *  useGitFollow). The PR half is left to the `pr:*` events. A failed read
   *  keeps the last-known state. */
  loadGitState(agentId: string): Promise<void>;
  /** The Ship tab's one read when it opens with no checks cached: PR state and
   *  CI from one `get_pr_live` pass, so both are from the same moment. The
   *  host's `pr:*` events keep them current after that. A null `checks` means
   *  the CI read didn't resolve (see `PrLive`), so the last tint is kept rather
   *  than blanked. */
  loadPrLive(agentId: string): Promise<void>;
  /** Read the PR's unresolved review threads — once, when the Ship tab opens
   *  with none cached; `pr:threads_changed` follows. A no-op on a host without
   *  `get_pr_threads`. */
  loadPrThreads(agentId: string): Promise<void>;
  /** Refresh the fleet-wide working-tree stats behind the agent rows. */
  loadShortstats(): Promise<void>;
  /** Seed the fleet-wide PR state behind the agent rows from one
   *  `get_all_pr_status` read, on every handshake and return to the
   *  foreground; the host watcher's `pr:*` events keep it current between.
   *  Primary-repo entries only (the phone is single-repo for now); a null
   *  `checks` keeps the last tint, as in `loadPrLive`, and an agent absent from
   *  the map keeps everything — absence means "nothing to say", not "no PR". A
   *  no-op on a host without the op. */
  loadPrStatus(): Promise<void>;
}

/** What the slice borrows from the store it composes into, so it can be a module
 *  of its own without importing the store back. */
export interface GitPrDeps {
  /** The link of the host the slice acts on. */
  link: HostLink;
  /** That host's PR write order, shared with its event handlers. */
  prWrites: PrWriteOrder;
}

export function createGitPrSlice(set: Set, get: Get, deps: GitPrDeps): GitPrSlice {
  const { link, prWrites } = deps;
  const { api } = link;

  /** Per agent, the order of its git reads and the one out now. Two readers
   *  share a checkout, the opener (`loadGit`) and the follow poll, and over the
   *  relay a read can outlast a tick: a call while one is out joins it rather
   *  than stacking another, and only the newest read writes. */
  const gitReads = new Map<string, ReturnType<typeof newestWins>>();
  const gitReadsInFlight = new Map<string, Promise<void>>();
  const claimGitRead = (agentId: string) => {
    let order = gitReads.get(agentId);
    if (!order) {
      order = newestWins();
      gitReads.set(agentId, order);
    }
    return order.claim();
  };

  return {
    gitStates: {},
    shortstats: {},
    prStates: {},
    prChecks: {},
    prComments: {},

    async loadGit(agentId) {
      // The PR halves are ordered against the watcher's events like every PR
      // read here (see `loadPrLive`); the git state has no event to race.
      const ticket = prWrites.issue();
      try {
        const [, pr] = await Promise.all([get().loadGitState(agentId), api.getPrState(agentId)]);
        if (prWrites.accept("prStates", agentId, ticket)) {
          set((s) => ({ prStates: { ...s.prStates, [agentId]: pr } }));
        }
        if (pr) {
          const checks = await api.getPrChecks(agentId);
          if (!prWrites.accept("prChecks", agentId, ticket)) return;
          set((s) => ({ prChecks: { ...s.prChecks, [agentId]: checks } }));
        }
      } catch {
        // Git/PR reads are advisory; leave the last-known values alone.
      }
    },

    loadGitState(agentId) {
      const out = gitReadsInFlight.get(agentId);
      if (out) return out;
      const claim = claimGitRead(agentId);
      const read = (async () => {
        try {
          const git = await api.getGitState(agentId);
          if (claim.current()) set((s) => ({ gitStates: { ...s.gitStates, [agentId]: git } }));
        } catch {
          // Advisory, like loadGit: keep the last-known state.
        } finally {
          gitReadsInFlight.delete(agentId);
        }
      })();
      gitReadsInFlight.set(agentId, read);
      return read;
    },

    async loadPrLive(agentId) {
      // Claimed before asking: a `pr:*` event that lands while the read is out
      // is newer than what the read saw, and the watcher, having sent it, will
      // not send it again — so the reply must not land on top of it
      // (@desktop/store/prWriteOrder).
      const ticket = prWrites.issue();
      try {
        const live = await api.getPrLive(agentId);
        // Null means the host had nothing to say this round, not "no PR".
        if (!live) return;
        // One ticket, checked per slice: a checks event outranks only the checks.
        const takeState = prWrites.accept("prStates", agentId, ticket);
        const takeChecks = !!live.checks && prWrites.accept("prChecks", agentId, ticket);
        set((s) => ({
          ...(takeState ? { prStates: { ...s.prStates, [agentId]: live.state } } : {}),
          ...(takeChecks ? { prChecks: { ...s.prChecks, [agentId]: live.checks } } : {}),
        }));
      } catch {
        // Advisory, like loadGit: keep the last-known state.
      }
    },

    async loadPrThreads(agentId) {
      if (!get().hostSupports("get_pr_threads")) return;
      // Ordered against `pr:threads_changed`, as `loadPrLive` is against the rest.
      const ticket = prWrites.issue();
      try {
        const comments = await api.getPrThreads(agentId);
        if (!prWrites.accept("prComments", agentId, ticket)) return;
        set((s) => ({ prComments: { ...s.prComments, [agentId]: comments } }));
      } catch {
        // Advisory, like loadGit: keep the last-known threads.
      }
    },

    async loadShortstats() {
      try {
        // Replaced wholesale, mirroring the desktop's `fetchAllShortstats`: the
        // host answers for the whole fleet, so an agent that has gone clean (or
        // been archived) falls out instead of keeping its last number.
        const shortstats = await api.getAllShortstats();
        set({ shortstats });
      } catch {
        // Advisory, like the rest of the git reads: keep the last numbers.
      }
    },

    async loadPrStatus() {
      if (!get().hostSupports("get_all_pr_status")) return;
      // Ordered per agent, row by row: an event for one agent while the sweep is
      // out outranks that agent's row and nobody else's (see `loadPrLive`).
      const ticket = prWrites.issue();
      try {
        const all = await api.getAllPrStatus();
        const states: Record<string, PrState | null> = {};
        const checks: Record<string, PrChecks> = {};
        for (const [key, entry] of Object.entries(all)) {
          // `"{agentId}::{subdir}"` keys are secondary repos, which no screen
          // here shows yet.
          if (key.includes("::")) continue;
          if (prWrites.accept("prStates", key, ticket)) states[key] = entry.state;
          if (entry.checks && prWrites.accept("prChecks", key, ticket)) checks[key] = entry.checks;
        }
        set((s) => ({
          prStates: { ...s.prStates, ...states },
          prChecks: { ...s.prChecks, ...checks },
        }));
      } catch {
        // Advisory, like loadShortstats: keep the last-known state.
      }
    },
  };
}
