import { useEffect, useMemo, useState } from "react";
import type { AgentRecord } from "../../api";
import { useAppStore } from "../../store";
import { Icon } from "../Icon";
import { basename } from "../../util/format";
import { SidebarHeader } from "./SidebarHeader";
import { SidebarFooter } from "./SidebarFooter";
import { ProjectGroup } from "./ProjectGroup";
import { NewProjectPopover } from "./NewProjectPopover";
import { NewProject, type NewProjectMode } from "../NewProject";

interface RepoGroup {
  repoPath: string;
  agents: AgentRecord[];
  pinned: boolean;
}

/** Build groups from (a) pinned repos and (b) any repo referenced by an
 *  existing agent. Drafts are excluded — they surface in the sidebar only once
 *  spawned into real agents. */
function groupByRepo(
  pinned: string[],
  agents: readonly AgentRecord[],
): RepoGroup[] {
  const map = new Map<string, RepoGroup>();
  for (const p of pinned) {
    map.set(p, { repoPath: p, agents: [], pinned: true });
  }
  for (const a of agents) {
    const primary = a.repos[0]?.repo_path;
    if (!primary) continue;
    const existing = map.get(primary);
    if (existing) existing.agents.push(a);
    else map.set(primary, { repoPath: primary, agents: [a], pinned: false });
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
    }))
    .filter(
      (g) =>
        g.agents.length > 0 ||
        basename(g.repoPath).toLowerCase().includes(needle),
    );
}

export function Sidebar() {
  const workspace = useAppStore((s) => s.workspace);
  const selectedAgentId = useAppStore((s) => s.selectedAgentId);

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
  // Drafts are intentionally excluded: an in-progress draft lives only in the
  // center pane and joins the sidebar (as a real agent) once its first message
  // spawns it.
  const groups = useMemo(
    () => groupByRepo(workspace?.repos ?? [], liveAgents),
    [workspace?.repos, liveAgents],
  );
  const filtered = useMemo(() => applySearch(groups, query), [groups, query]);

  // Auto-expand a project when its agent is selected.
  useEffect(() => {
    setOpenMap((prev) => {
      const next = { ...prev };
      for (const g of groups) {
        if (g.agents.some((a) => a.id === selectedAgentId)) next[g.repoPath] = true;
        if (!(g.repoPath in next)) {
          next[g.repoPath] = g.agents.length > 0;
        }
      }
      return next;
    });
  }, [groups, selectedAgentId]);

  return (
    <>
      <SidebarHeader query={query} onChange={setQuery} />
      <div className="side-scroll">
        <div className="side-section">
          <button
            className="add-proj-cta"
            onClick={() => setNpOpen(true)}
            aria-label="Add project"
          >
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
                open={openMap[g.repoPath] ?? false}
                removable={g.pinned && g.agents.length === 0}
                onToggle={() =>
                  setOpenMap((m) => ({ ...m, [g.repoPath]: !m[g.repoPath] }))
                }
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
