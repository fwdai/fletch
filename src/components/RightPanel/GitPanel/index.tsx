import { useMemo, useState } from "react";
import { useShallow } from "zustand/react/shallow";
import type { AgentRecord, TrackedRepo } from "@/api";
import { useAppStore } from "@/store";
import { checkoutKey } from "@/store/git";
import { basename } from "@/util/format";
import { prSnapshot } from "@/util/prState";
import { GitRepoSection } from "./GitRepoSection";
import { buildLanes, defaultLane, type LaneScope } from "./laneModel";

/** A repo of the agent plus the scope its section reads/writes under:
 *  `subdir` is undefined for the primary repo (index 0 — plain agent-keyed
 *  state, shared with live events and bulk polls) and the checkout's
 *  directory name for secondaries. */
interface RepoScope {
  repo: TrackedRepo;
  subdir?: string;
}

const scopesFor = (agent: AgentRecord): RepoScope[] =>
  agent.repos.map((repo, i) => ({ repo, subdir: i === 0 ? undefined : repo.subdir }));

/** State-aware git panel driven by live git state from the Tauri backend: one
 *  header of lanes, one ladder, one footer, whatever the number of repos.
 *
 *  The lanes come from every checkout of the agent — the primary always, a
 *  secondary once it has anything git-worthy to show (uncommitted changes, a
 *  named branch, a bound PR, or an in-flight delegation, which pins it so its
 *  progress doesn't vanish mid-flight). The section below describes the lane
 *  in view and acts on its checkout. */
export function GitPanel({ agent }: { agent: AgentRecord }) {
  const scopes = useMemo(() => scopesFor(agent), [agent]);
  const keys = useMemo(
    () => scopes.map((sc) => checkoutKey(agent.id, sc.subdir)),
    [scopes, agent.id],
  );
  // Per-key references, shallow-compared, so the lanes rebuild only when one
  // of these checkouts actually changes.
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

  // Which lane the ladder describes: the user's pick while it exists, else the
  // default. Local to the panel — viewing a lane never moves the host's focus.
  // The pick remembers which agent it was made for, so a switch to another
  // workspace starts from that one's default rather than a stale lane id.
  const [pick, setPick] = useState<{ agentId: string; laneId: string } | null>(null);
  const picked = pick?.agentId === agent.id ? pick.laneId : null;
  const viewed = (picked ? lanes.find((l) => l.id === picked) : undefined) ?? defaultLane(lanes);
  // The checkout the viewed lane belongs to — the primary unless scoped.
  const scope = scopes.find((sc) => sc.repo.subdir === viewed?.scope?.id) ?? scopes[0];

  return (
    <GitRepoSection
      agent={agent}
      repo={scope?.repo}
      subdir={scope?.subdir}
      lanes={lanes}
      viewed={viewed}
      onView={(laneId) => setPick({ agentId: agent.id, laneId })}
    />
  );
}
