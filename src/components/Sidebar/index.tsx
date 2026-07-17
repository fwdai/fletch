import { useEffect, useMemo, useRef, useState } from "react";
import type { AgentRecord, ProjectRef, WfRun } from "@/api";
import { Icon } from "@/components/Icon";
import { NewProject, type NewProjectMode } from "@/components/NewProject";
import type { DraftAgent } from "@/store";
import { useAppStore } from "@/store";
import { basename } from "@/util/format";
import { useRuns } from "@/workflows/run/useRuns";
import { NewProjectPopover } from "./NewProjectPopover";
import { ProjectGroup } from "./ProjectGroup";
import { SidebarFooter } from "./SidebarFooter";
import { SidebarHeader } from "./SidebarHeader";
import { useProjectReorder } from "./useProjectReorder";

interface ProjectGroupData {
  /** Stable group id: the project_id, or the repo path for repos that aren't
   *  pinned (an agent whose repo was removed from the sidebar). */
  key: string;
  /** Project display name (folder basename fallback for unpinned repos). */
  label: string;
  /** All repos attached to the project, in creation order. */
  repoPaths: string[];
  /** The project's first repo — the drag/order key, draft target, and
   *  settings key, so single-repo projects behave exactly as before. */
  primaryPath: string;
  agents: AgentRecord[];
  drafts: DraftAgent[];
  runs: WfRun[];
  pinned: boolean;
}

/** Build one group per project from (a) pinned repos (each carries its
 *  project via ProjectRef; a multi-repo project folds into one group),
 *  (b) any repo referenced by an existing agent, draft, or workflow run —
 *  those resolve to their project's group, or a path-keyed fallback group
 *  when the repo isn't pinned. */
function groupByProject(
  refs: readonly ProjectRef[],
  agents: readonly AgentRecord[],
  drafts: readonly DraftAgent[],
  runs: readonly WfRun[],
): ProjectGroupData[] {
  const groups = new Map<string, ProjectGroupData>();
  const byPath = new Map<string, ProjectGroupData>();
  for (const ref of refs) {
    const key = ref.project_id || ref.path;
    let g = groups.get(key);
    if (!g) {
      g = {
        key,
        label: ref.name,
        repoPaths: [],
        primaryPath: ref.path,
        agents: [],
        drafts: [],
        runs: [],
        pinned: true,
      };
      groups.set(key, g);
    }
    g.repoPaths.push(ref.path);
    byPath.set(ref.path, g);
  }
  const ensure = (p: string): ProjectGroupData => {
    let g = byPath.get(p);
    if (!g) {
      g = {
        key: p,
        label: basename(p),
        repoPaths: [p],
        primaryPath: p,
        agents: [],
        drafts: [],
        runs: [],
        pinned: false,
      };
      groups.set(p, g);
      byPath.set(p, g);
    }
    return g;
  };
  for (const a of agents) {
    const primary = a.repos[0]?.repo_path;
    if (primary) ensure(primary).agents.push(a);
  }
  for (const d of drafts) ensure(d.repoPath).drafts.push(d);
  for (const r of runs) ensure(r.repo_path).runs.push(r);
  return Array.from(groups.values());
}

function applySearch(groups: ProjectGroupData[], q: string): ProjectGroupData[] {
  if (!q.trim()) return groups;
  const needle = q.toLowerCase();
  const groupMatches = (g: ProjectGroupData) =>
    g.label.toLowerCase().includes(needle) ||
    g.repoPaths.some((p) => basename(p).toLowerCase().includes(needle));
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
      // Run rows render their own body; keep them when the project matches.
      runs: groupMatches(g) ? g.runs : [],
    }))
    .filter(
      (g) => g.agents.length > 0 || g.drafts.length > 0 || g.runs.length > 0 || groupMatches(g),
    );
}

export function Sidebar() {
  const workspace = useAppStore((s) => s.workspace);
  const drafts = useAppStore((s) => s.drafts);
  const selectedAgentId = useAppStore((s) => s.selectedAgentId);
  const activeDraftId = useAppStore((s) => s.activeDraftId);
  const selectedRunId = useAppStore((s) => s.selectedRunId);

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
  // Tears down the in-flight drag's window listeners and resets state. Held in a
  // ref so an interrupted drag (pointercancel, or the sidebar unmounting) can
  // clean up too — not just a normal pointerup.
  const dragCleanup = useRef<(() => void) | null>(null);

  const { sortPaths, reorder } = useProjectReorder();

  const liveAgents = useMemo(
    () =>
      (workspace?.agents ?? [])
        .filter((a) => !a.archive)
        .slice()
        .sort((a, b) => (a.created_at < b.created_at ? 1 : -1)),
    [workspace?.agents],
  );
  const runs = useRuns();
  const groups = useMemo(() => {
    const built = groupByProject(workspace?.projects ?? [], liveAgents, drafts, runs);
    // Manual ordering is keyed by each project's primary repo path, so orders
    // saved before multi-repo grouping keep working unchanged.
    const order = sortPaths(built.map((g) => g.primaryPath));
    const byPath = new Map(built.map((g) => [g.primaryPath, g]));
    return order.map((p) => byPath.get(p)).filter((g): g is ProjectGroupData => g !== undefined);
  }, [workspace?.projects, liveAgents, drafts, runs, sortPaths]);
  const filtered = useMemo(() => applySearch(groups, query), [groups, query]);

  // Reordering is only meaningful over the full, unfiltered list.
  const reorderable = !query.trim();
  const orderedPaths = useMemo(() => groups.map((g) => g.primaryPath), [groups]);

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
    // `commit` is true only for a clean pointerup; a cancel or unmount tears the
    // drag down without reordering.
    // `commit` is true only for a clean pointerup; a cancel, focus loss, or
    // unmount tears the drag down without reordering.
    const finish = (commit: boolean) => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      window.removeEventListener("pointercancel", onCancel);
      window.removeEventListener("blur", onCancel);
      dragCleanup.current = null;
      const info = dragInfo.current;
      dragInfo.current = null;
      if (commit && info?.active && info.over && info.over !== info.path) {
        reorder(paths, info.path, info.over);
      }
      setDragPath(null);
      setOverPath(null);
    };
    const onUp = () => finish(true);
    const onCancel = () => finish(false);

    dragCleanup.current = () => finish(false);
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
    window.addEventListener("pointercancel", onCancel);
    // The webview can drop the pointer stream on focus loss without a
    // pointercancel; bail out so the drag can't get stuck.
    window.addEventListener("blur", onCancel);
  }

  // Safety net: if the sidebar unmounts mid-drag, tear down the window listeners
  // so they don't leak past this component's life.
  useEffect(() => () => dragCleanup.current?.(), []);

  // Auto-expand a project when its agent, draft, or run is selected.
  useEffect(() => {
    setOpenMap((prev) => {
      const next = { ...prev };
      for (const g of groups) {
        if (g.agents.some((a) => a.id === selectedAgentId)) next[g.key] = true;
        if (g.drafts.some((d) => d.id === activeDraftId)) next[g.key] = true;
        if (g.runs.some((r) => r.id === selectedRunId)) next[g.key] = true;
        if (!(g.key in next)) {
          next[g.key] = g.agents.length > 0 || g.drafts.length > 0 || g.runs.length > 0;
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
              const isOver =
                reorderable && overPath === g.primaryPath && dragPath !== g.primaryPath;
              const dropAfter =
                isOver &&
                dragPath != null &&
                orderedPaths.indexOf(dragPath) < orderedPaths.indexOf(g.primaryPath);
              return (
                <ProjectGroup
                  key={g.key}
                  label={g.label}
                  repoPath={g.primaryPath}
                  repoPaths={g.repoPaths}
                  agents={g.agents}
                  drafts={g.drafts}
                  runs={g.runs}
                  open={openMap[g.key] ?? false}
                  onToggle={() => setOpenMap((m) => ({ ...m, [g.key]: !m[g.key] }))}
                  reorderable={reorderable}
                  dragging={dragPath === g.primaryPath}
                  dropIndicator={isOver ? (dropAfter ? "after" : "before") : null}
                  onReorderPointerDown={
                    reorderable
                      ? (e, markDragged) => startReorder(g.primaryPath, e, markDragged)
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
