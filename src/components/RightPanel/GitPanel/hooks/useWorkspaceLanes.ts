import { useMemo } from "react";
import { useShallow } from "zustand/react/shallow";
import type { AgentRecord, TrackedRepo } from "@/api";
import { useAppStore } from "@/store";
import { checkoutKey } from "@/store/git";
import { basename } from "@/util/format";
import { prSnapshot } from "@/util/prState";
import { buildLanes, type Lane, type LaneScope } from "../laneModel";

/** A repo of the agent plus the scope its state is keyed under: `subdir` is
 *  undefined for the primary repo (index 0 — plain agent-keyed state, shared
 *  with live events and bulk polls) and the checkout's directory name for
 *  secondaries. */
export interface RepoScope {
  repo: TrackedRepo;
  subdir?: string;
}

/** The workspace's lanes, from every checkout of the agent — the primary
 *  always, a secondary once it has anything git-worthy to show (uncommitted
 *  changes, a named branch, a bound PR, or an in-flight delegation, which pins
 *  it so its progress doesn't vanish mid-flight). The one derivation the Git
 *  panel and the title bar popover both draw, so they cannot list different
 *  rows. Per-key store reads, shallow-compared, so the lanes rebuild only when
 *  one of these checkouts actually changes. */
export function useWorkspaceLanes(agent: AgentRecord): { lanes: Lane[]; scopes: RepoScope[] } {
  const scopes = useMemo<RepoScope[]>(
    () => agent.repos.map((repo, i) => ({ repo, subdir: i === 0 ? undefined : repo.subdir })),
    [agent.repos],
  );
  const keys = useMemo(
    () => scopes.map((sc) => checkoutKey(agent.id, sc.subdir)),
    [scopes, agent.id],
  );
  const gits = useAppStore(useShallow((s) => keys.map((k) => s.gitStates[k] ?? null)));
  // Keep an absent key (never fetched) distinct from a confirmed null: only the
  // former falls back to the repo's persisted snapshot (see `useGitPanelData`).
  const prs = useAppStore(
    useShallow((s) => keys.map((k) => (k in s.prStates ? s.prStates[k] : undefined))),
  );
  const checks = useAppStore(useShallow((s) => keys.map((k) => s.prChecks[k] ?? null)));
  const sets = useAppStore(useShallow((s) => keys.map((k) => s.prSets[k])));
  const delegated = useAppStore(useShallow((s) => keys.map((k) => s.delegations[k] != null)));

  const lanes = useMemo(() => {
    const multi = scopes.length > 1;
    return scopes.flatMap((sc, i) => {
      const focused = prs[i] !== undefined ? prs[i] : prSnapshot(sc.repo);
      const active =
        i === 0 ||
        delegated[i] ||
        (gits[i]?.files.length ?? 0) > 0 ||
        Boolean(sc.repo.branch) ||
        sc.repo.pr_number != null;
      if (!active) return [];
      const scope: LaneScope | undefined = multi
        ? {
            id: sc.repo.subdir,
            subdir: sc.subdir,
            label: sc.repo.label ?? basename(sc.repo.repo_path),
          }
        : undefined;
      return buildLanes(sets[i], focused, checks[i], gits[i], scope, sc.repo.branch);
    });
  }, [scopes, gits, prs, checks, sets, delegated]);

  return { lanes, scopes };
}

/** The checkout a lane belongs to — the primary unless the lane is scoped. */
export function scopeOf(lane: Lane | null, scopes: RepoScope[]): RepoScope | undefined {
  return scopes.find((sc) => sc.repo.subdir === lane?.scope?.id) ?? scopes[0];
}
