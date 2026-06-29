import { useEffect, useMemo, useState } from "react";
import type { AgentRecord } from "../../api";
import type { DraftAgent } from "../../store";
import { useAppStore } from "../../store";
import { basename } from "../../util/format";
import type { WorkflowRun } from "../../workflows/run/types";
import { useRuns } from "../../workflows/run/useRuns";
import { Icon } from "../Icon";
import { NewProject, type NewProjectMode } from "../NewProject";
import { NewProjectPopover } from "./NewProjectPopover";
import { ProjectGroup } from "./ProjectGroup";
import { SidebarFooter } from "./SidebarFooter";
import { SidebarHeader } from "./SidebarHeader";

interface RepoGroup {
  repoPath: string;
  agents: AgentRecord[];
  drafts: DraftAgent[];
  runs: WorkflowRun[];
  pinned: boolean;
}

/** Build groups from (a) pinned repos, (b) any repo referenced by an
 *  existing agent, (c) any repo referenced by a draft, (d) any workflow run. */
function groupByRepo(
  pinned: string[],
  agents: readonly AgentRecord[],
  drafts: readonly DraftAgent[],
  runs: readonly WorkflowRun[],
): RepoGroup[] {
  const map = new Map<string, RepoGroup>();
  const ensure = (p: string, pinnedFlag = false): RepoGroup => {
    let g = map.get(p);
    if (!g) {
      g = { repoPath: p, agents: [], drafts: [], runs: [], pinned: pinnedFlag };
      map.set(p, g);
    }
    return g;
  };
  for (const p of pinned) ensure(p, true);
  for (const a of agents) {
    const primary = a.repos[0]?.repo_path;
    if (primary) ensure(primary).agents.push(a);
  }
  for (const d of drafts) ensure(d.repoPath).drafts.push(d);
  for (const r of runs) ensure(r.repo_path).runs.push(r);
  return Array.from(map.values());
}

function applySearch(groups: RepoGroup[], q: string): RepoGroup[] {
  if (!q.trim()) return groups;
  const needle = q.toLowerCase();
  return groups
    .map((g) => ({
      ...g,
      agents: g.agents.filter(
        (a) =>
          a.name.toLowerCase().includes(needle) ||
          a.task.toLowerCase().includes(needle) ||
          a.repos[0]?.branch?.toLowerCase().includes(needle),
      ),
      drafts: g.drafts.filter((d) => d.name.toLowerCase().includes(needle)),
      // Run rows render their own body; keep them when the repo matches.
      runs: basename(g.repoPath).toLowerCase().includes(needle) ? g.runs : [],
    }))
    .filter(
      (g) =>
        g.agents.length > 0 ||
        g.drafts.length > 0 ||
        g.runs.length > 0 ||
        basename(g.repoPath).toLowerCase().includes(needle),
    );
}

export function Sidebar() {
  const workspace = useAppStore((s) => s.workspace);
  const drafts = useAppStore((s) => s.drafts);
  const selectedAgentId = useAppStore((s) => s.selectedAgentId);
  const selectedRunId = useAppStore((s) => s.selectedRunId);
  const activeDraftId = useAppStore((s) => s.activeDraftId);

  const [query, setQuery] = useState("");
  const [openMap, setOpenMap] = useState<Record<string, boolean>>({});
  const [npOpen, setNpOpen] = useState(false);
  const [npMode, setNpMode] = useState<NewProjectMode | null>(null);

  const liveAgents = useMemo(
    () =>
      (workspace?.agents ?? [])
        .filter((a) => !a.archive)
        .slice()
        .sort((a, b) => (a.created_at < b.created_at ? 1 : -1)),
    [workspace?.agents],
  );
  const runs = useRuns();
  const groups = useMemo(
    () => groupByRepo(workspace?.repos ?? [], liveAgents, drafts, runs),
    [workspace?.repos, liveAgents, drafts, runs],
  );
  const filtered = useMemo(() => applySearch(groups, query), [groups, query]);

  // Auto-expand a project when its agent, draft, or run is selected.
  useEffect(() => {
    setOpenMap((prev) => {
      const next = { ...prev };
      for (const g of groups) {
        if (g.agents.some((a) => a.id === selectedAgentId)) next[g.repoPath] = true;
        if (g.drafts.some((d) => d.id === activeDraftId)) next[g.repoPath] = true;
        if (g.runs.some((r) => r.id === selectedRunId)) next[g.repoPath] = true;
        if (!(g.repoPath in next)) {
          next[g.repoPath] = g.agents.length > 0 || g.drafts.length > 0 || g.runs.length > 0;
        }
      }
      return next;
    });
  }, [groups, selectedAgentId, activeDraftId, selectedRunId]);

  return (
    <>
      <SidebarHeader query={query} onChange={setQuery} />
      <div className="side-scroll">
        <div className="side-section">
          <button className="add-proj-cta" onClick={() => setNpOpen(true)} aria-label="Add project">
            <Icon name="plus" size={13} />
            <span>Add project</span>
          </button>

          {filtered.length === 0 ? (
            <div className="empty-msg" style={{ padding: "28px 12px" }}>
              <div className="et">{query ? "No matches" : "No projects yet"}</div>
              <div>{query ? "Try a different search." : "Add a repo to get started."}</div>
            </div>
          ) : (
            filtered.map((g) => (
              <ProjectGroup
                key={g.repoPath}
                label={basename(g.repoPath)}
                repoPath={g.repoPath}
                agents={g.agents}
                drafts={g.drafts}
                runs={g.runs}
                open={openMap[g.repoPath] ?? false}
                removable={
                  g.pinned && g.agents.length === 0 && g.drafts.length === 0 && g.runs.length === 0
                }
                onToggle={() => setOpenMap((m) => ({ ...m, [g.repoPath]: !m[g.repoPath] }))}
              />
            ))
          )}
        </div>
      </div>

      <SidebarFooter />

      {npOpen && (
        <NewProjectPopover
          onClose={() => setNpOpen(false)}
          onChoose={(mode) => {
            setNpOpen(false);
            setNpMode(mode);
          }}
        />
      )}
      {npMode && <NewProject mode={npMode} onClose={() => setNpMode(null)} />}
    </>
  );
}
