import { useEffect, useMemo, useState } from "react";
import type { AgentRecord } from "../../api";
import type { DraftAgent } from "../../store";
import { useAppStore } from "../../store";
import { basename } from "../../util/format";
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
  pinned: boolean;
}

/** Build groups from (a) pinned repos, (b) any repo referenced by an
 *  existing agent, (c) any repo referenced by a draft. */
function groupByRepo(
  pinned: string[],
  agents: readonly AgentRecord[],
  drafts: readonly DraftAgent[],
): RepoGroup[] {
  const map = new Map<string, RepoGroup>();
  for (const p of pinned) {
    map.set(p, { repoPath: p, agents: [], drafts: [], pinned: true });
  }
  for (const a of agents) {
    const primary = a.repos[0]?.repo_path;
    if (!primary) continue;
    const existing = map.get(primary);
    if (existing) existing.agents.push(a);
    else map.set(primary, { repoPath: primary, agents: [a], drafts: [], pinned: false });
  }
  for (const d of drafts) {
    const existing = map.get(d.repoPath);
    if (existing) existing.drafts.push(d);
    else map.set(d.repoPath, { repoPath: d.repoPath, agents: [], drafts: [d], pinned: false });
  }
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
    }))
    .filter(
      (g) =>
        g.agents.length > 0 ||
        g.drafts.length > 0 ||
        basename(g.repoPath).toLowerCase().includes(needle),
    );
}

export function Sidebar() {
  const workspace = useAppStore((s) => s.workspace);
  const drafts = useAppStore((s) => s.drafts);
  const selectedAgentId = useAppStore((s) => s.selectedAgentId);
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
  const groups = useMemo(
    () => groupByRepo(workspace?.repos ?? [], liveAgents, drafts),
    [workspace?.repos, liveAgents, drafts],
  );
  const filtered = useMemo(() => applySearch(groups, query), [groups, query]);

  // Auto-expand a project when its agent or draft is selected.
  useEffect(() => {
    setOpenMap((prev) => {
      const next = { ...prev };
      for (const g of groups) {
        if (g.agents.some((a) => a.id === selectedAgentId)) next[g.repoPath] = true;
        if (g.drafts.some((d) => d.id === activeDraftId)) next[g.repoPath] = true;
        if (!(g.repoPath in next)) {
          next[g.repoPath] = g.agents.length > 0 || g.drafts.length > 0;
        }
      }
      return next;
    });
  }, [groups, selectedAgentId, activeDraftId]);

  return (
    <>
      <SidebarHeader query={query} onChange={setQuery} />
      <div className="side-scroll">
        <div className="side-section">
          <button className="add-proj-cta flex-center" onClick={() => setNpOpen(true)} aria-label="Add project">
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
                open={openMap[g.repoPath] ?? false}
                removable={g.pinned && g.agents.length === 0 && g.drafts.length === 0}
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
