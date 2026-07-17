import { useCallback, useEffect, useRef } from "react";
import type { AgentRecord, GitState, TrackedRepo } from "@/api";
import { useAppStore } from "@/store";
import { gitKey } from "@/store/git";
import { basename } from "@/util/format";
import { usePoll } from "@/util/hooks";
import { GitRepoSection } from "./GitRepoSection";

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

/** A repo earns its own section once it has anything git-worthy to show:
 *  uncommitted changes, a named branch, or a bound PR. */
const repoActive = (scope: RepoScope, gitStates: Record<string, GitState>, agentId: string) =>
  (gitStates[gitKey(agentId, scope.subdir)]?.files.length ?? 0) > 0 ||
  Boolean(scope.repo.branch) ||
  scope.repo.pr_number != null;

/** State-aware git panel driven by live git state from the Tauri backend.
 *  Single-repo agents render one `GitRepoSection` — exactly the panel as it
 *  always was. Multi-repo agents render one section per ACTIVE repo (changes,
 *  branch, or PR), each under a slim repo-name header; when nothing is active
 *  yet, just the primary repo's section (its empty state) without a header. */
export function GitPanel({ agent }: { agent: AgentRecord }) {
  if (agent.repos.length <= 1) {
    return <GitRepoSection agent={agent} repo={agent.repos[0]} />;
  }
  return <MultiRepoGitPanel agent={agent} />;
}

function MultiRepoGitPanel({ agent }: { agent: AgentRecord }) {
  const fetchGitState = useAppStore((s) => s.fetchGitState);
  const gitStates = useAppStore((s) => s.gitStates);

  const scopes = scopesFor(agent);
  const active = scopes.filter((sc) => repoActive(sc, gitStates, agent.id));
  // Nothing active yet → the primary repo's section (today's empty state),
  // with no section header.
  const sections = active.length > 0 ? active : [scopes[0]];

  // On mount / agent change, fetch git state for EVERY repo so "active" can be
  // computed — rendered sections then keep their own 1s poll going.
  const repos = agent.repos;
  useEffect(() => {
    repos.forEach((repo, i) => void fetchGitState(agent.id, i === 0 ? undefined : repo.subdir));
  }, [agent.id, repos, fetchGitState]);

  // Repos without a rendered section have no per-section poll, so sweep them
  // on a slow cadence — an agent touching a dormant repo promotes it to a
  // section within a tick. Read through a ref so the poll's identity is
  // stable (the active set changes with every gitStates write).
  const dormantRef = useRef<RepoScope[]>([]);
  dormantRef.current =
    active.length > 0 ? scopes.filter((sc) => !active.includes(sc)) : scopes.slice(1);
  const pollDormant = useCallback(async () => {
    await Promise.all(dormantRef.current.map((sc) => fetchGitState(agent.id, sc.subdir)));
  }, [agent.id, fetchGitState]);
  usePoll(pollDormant, 5000, [pollDormant]);

  return (
    <div className="git-multi">
      {sections.map((sc) => (
        <section key={sc.repo.subdir} className="git-repo-sect">
          {active.length > 0 && (
            <div className="git-repo-name text-xs">
              {sc.repo.label ?? basename(sc.repo.repo_path)}
            </div>
          )}
          <GitRepoSection agent={agent} repo={sc.repo} subdir={sc.subdir} />
        </section>
      ))}
    </div>
  );
}
