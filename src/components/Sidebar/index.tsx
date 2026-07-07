import { useEffect, useMemo, useRef, useState } from "react";
import type { AgentRecord } from "@/api";
import { Icon } from "@/components/Icon";
import { NewProject, type NewProjectMode } from "@/components/NewProject";
import type { DraftAgent } from "@/store";
import { useAppStore } from "@/store";
import { basename } from "@/util/format";
import { NewProjectPopover } from "./NewProjectPopover";
import { ProjectGroup } from "./ProjectGroup";
import { SidebarFooter } from "./SidebarFooter";
import { SidebarHeader } from "./SidebarHeader";
import { useProjectReorder } from "./useProjectReorder";

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
  // Transient drag state for reordering: the group being dragged and the one
  // currently hovered as a drop target. Driven by pointer events (not the HTML5
  // drag-and-drop API, which Tauri's OS-level drag-drop handler swallows inside
  // the macOS webview — that handler stays on for the composer's file drop).
  const [dragPath, setDragPath] = useState<string | null>(null);
  const [overPath, setOverPath] = useState<string | null>(null);
  const dragInfo = useRef<{
    path: string;
    x: number;
    y: number;
    active: boolean;
    over: string | null;
  } | null>(null);

  const { sortPaths, reorder } = useProjectReorder();

  const liveAgents = useMemo(
    () =>
      (workspace?.agents ?? [])
        .filter((a) => !a.archive)
        .slice()
        .sort((a, b) => (a.created_at < b.created_at ? 1 : -1)),
    [workspace?.agents],
  );
  const groups = useMemo(() => {
    const built = groupByRepo(workspace?.repos ?? [], liveAgents, drafts);
    const order = sortPaths(built.map((g) => g.repoPath));
    const byPath = new Map(built.map((g) => [g.repoPath, g]));
    return order.map((p) => byPath.get(p)).filter((g): g is RepoGroup => g !== undefined);
  }, [workspace?.repos, liveAgents, drafts, sortPaths]);
  const filtered = useMemo(() => applySearch(groups, query), [groups, query]);

  // Reordering is only meaningful over the full, unfiltered list.
  const reorderable = !query.trim();
  const orderedPaths = useMemo(() => groups.map((g) => g.repoPath), [groups]);

  // Begin a pointer-driven reorder. `markDragged` lets the group swallow the
  // trailing click so a real drag doesn't also toggle it open/closed. The order
  // is captured up front — it doesn't change mid-drag.
  function startReorder(path: string, e: React.PointerEvent, markDragged: () => void) {
    const paths = orderedPaths;
    dragInfo.current = { path, x: e.clientX, y: e.clientY, active: false, over: null };

    const onMove = (ev: PointerEvent) => {
      const info = dragInfo.current;
      if (!info) return;
      // Only promote to a drag once the pointer clears a small threshold, so a
      // plain click still falls through to the toggle.
      if (!info.active) {
        if (Math.hypot(ev.clientX - info.x, ev.clientY - info.y) < 4) return;
        info.active = true;
        markDragged();
        setDragPath(info.path);
      }
      const target = document
        .elementFromPoint(ev.clientX, ev.clientY)
        ?.closest<HTMLElement>("[data-repo-path]");
      const over = target?.dataset.repoPath ?? null;
      info.over = over;
      setOverPath(over);
    };
    const onUp = () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      const info = dragInfo.current;
      dragInfo.current = null;
      if (info?.active && info.over && info.over !== info.path) {
        reorder(paths, info.path, info.over);
      }
      setDragPath(null);
      setOverPath(null);
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
  }

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
          <button
            className="add-proj-cta flex-center text-sm"
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
            filtered.map((g) => {
              const isOver = reorderable && overPath === g.repoPath && dragPath !== g.repoPath;
              const dropAfter =
                isOver &&
                dragPath != null &&
                orderedPaths.indexOf(dragPath) < orderedPaths.indexOf(g.repoPath);
              return (
                <ProjectGroup
                  key={g.repoPath}
                  label={basename(g.repoPath)}
                  repoPath={g.repoPath}
                  agents={g.agents}
                  drafts={g.drafts}
                  open={openMap[g.repoPath] ?? false}
                  removable={g.pinned && g.agents.length === 0 && g.drafts.length === 0}
                  onToggle={() => setOpenMap((m) => ({ ...m, [g.repoPath]: !m[g.repoPath] }))}
                  reorderable={reorderable}
                  dragging={dragPath === g.repoPath}
                  dropIndicator={isOver ? (dropAfter ? "after" : "before") : null}
                  onReorderPointerDown={
                    reorderable
                      ? (e, markDragged) => startReorder(g.repoPath, e, markDragged)
                      : undefined
                  }
                />
              );
            })
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
