import { useEffect, useMemo, useRef, useState } from "react";
import type { AgentRecord, WfRun } from "@/api";
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

interface RepoGroup {
  repoPath: string;
  agents: AgentRecord[];
  drafts: DraftAgent[];
  runs: WfRun[];
  pinned: boolean;
}

/** Build groups from (a) pinned repos, (b) any repo referenced by an
 *  existing agent, (c) any repo referenced by a draft, (d) any workflow run. */
function groupByRepo(
  pinned: string[],
  agents: readonly AgentRecord[],
  drafts: readonly DraftAgent[],
  runs: readonly WfRun[],
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

function applySearch(
  groups: RepoGroup[],
  q: string,
  labelOf: (path: string) => string,
): RepoGroup[] {
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
      runs: labelOf(g.repoPath).toLowerCase().includes(needle) ? g.runs : [],
    }))
    .filter(
      (g) =>
        g.agents.length > 0 ||
        g.drafts.length > 0 ||
        g.runs.length > 0 ||
        labelOf(g.repoPath).toLowerCase().includes(needle),
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
    const built = groupByRepo(workspace?.repos ?? [], liveAgents, drafts, runs);
    const order = sortPaths(built.map((g) => g.repoPath));
    const byPath = new Map(built.map((g) => [g.repoPath, g]));
    return order.map((p) => byPath.get(p)).filter((g): g is RepoGroup => g !== undefined);
  }, [workspace?.repos, liveAgents, drafts, runs, sortPaths]);
  // Custom display name per pinned repo. Groups derived only from an agent's
  // repo (never pinned) have no entry and fall back to the folder basename.
  const labelOf = useMemo(() => {
    const byPath = new Map((workspace?.projects ?? []).map((p) => [p.path, p.name]));
    return (path: string) => byPath.get(path) ?? basename(path);
  }, [workspace?.projects]);
  const filtered = useMemo(() => applySearch(groups, query, labelOf), [groups, query, labelOf]);

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
                  label={labelOf(g.repoPath)}
                  repoPath={g.repoPath}
                  agents={g.agents}
                  drafts={g.drafts}
                  runs={g.runs}
                  open={openMap[g.repoPath] ?? false}
                  removable={
                    g.pinned &&
                    g.agents.length === 0 &&
                    g.drafts.length === 0 &&
                    g.runs.length === 0
                  }
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
