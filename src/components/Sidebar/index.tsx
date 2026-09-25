import { useEffect, useMemo, useRef, useState } from "react";
import type { AgentRecord, ProjectRef, WfRun } from "@/api";
import { Icon } from "@/components/Icon";
import { NewProject, type NewProjectMode } from "@/components/NewProject";
import {
  loadProjectActivity,
  type ProjectActivity,
  stampProjectActivity,
} from "@/storage/projectActivity";
import type { DraftAgent } from "@/store";
import { useAppStore } from "@/store";
import { useAnyGate } from "@/store/capabilities";
import { arrowTarget } from "@/util/arrowNav";
import { basename } from "@/util/format";
import { useMinuteClock } from "@/util/hooks";
import { useRuns } from "@/workflows/run/useRuns";
import { isGroupOpen, type OpenMap } from "./groupOpen";
import { NewProjectPopover } from "./NewProjectPopover";
import { ProjectGroup } from "./ProjectGroup";
import { bubbleActive } from "./recentProjects";
import { focusRow, rowOf, visibleRows } from "./rowNav";
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

/** Build one group per project. The pinned repos (`workspace.projects`) are the
 *  single source of truth for which groups exist: a multi-repo project folds
 *  into one group, keyed by `project_id`. Agents, runs, and drafts only *attach*
 *  to those groups — they never fabricate one from a stale reference, so a
 *  deleted or relocated project can't reappear as a phantom sidebar group.
 *
 *  Agents and runs resolve by `project_id` first (it survives a relocate, where
 *  a run's stored `repo_path` and a draft's path both go stale), then by repo
 *  path, and only a genuinely *unpinned* repo (e.g. detached while an agent
 *  still runs in it) gets a path-keyed fallback group. Drafts carry no
 *  project_id, so they attach by path only and are dropped from the view when
 *  their repo is gone. */
function groupByProject(
  refs: readonly ProjectRef[],
  agents: readonly AgentRecord[],
  drafts: readonly DraftAgent[],
  runs: readonly WfRun[],
): ProjectGroupData[] {
  const groups = new Map<string, ProjectGroupData>();
  const byPath = new Map<string, ProjectGroupData>();
  const byProjectId = new Map<string, ProjectGroupData>();
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
      if (ref.project_id) byProjectId.set(ref.project_id, g);
    }
    g.repoPaths.push(ref.path);
    byPath.set(ref.path, g);
  }
  // Resolve real backend work (agents, runs) to its group: by project first, so
  // a relocate — which moves the repo path but keeps the project_id — attaches
  // to the moved group instead of forking. A repo that isn't pinned at all
  // still surfaces its work via a path-keyed fallback group.
  const groupFor = (projectId: string, path: string | undefined): ProjectGroupData | null => {
    const byId = projectId ? byProjectId.get(projectId) : undefined;
    if (byId) return byId;
    if (!path) return null;
    let g = byPath.get(path);
    if (!g) {
      g = {
        key: path,
        label: basename(path),
        repoPaths: [path],
        primaryPath: path,
        agents: [],
        drafts: [],
        runs: [],
        pinned: false,
      };
      groups.set(path, g);
      byPath.set(path, g);
    }
    return g;
  };
  for (const a of agents) groupFor(a.project_id, a.repos[0]?.repo_path)?.agents.push(a);
  for (const r of runs) groupFor(r.project_id, r.repo_path)?.runs.push(r);
  // Drafts are client-only and keyed solely by repo path. Attach to an existing
  // group but never create one: a draft left pointing at a deleted/relocated
  // repo is dropped from the view rather than resurrecting a phantom group.
  for (const d of drafts) byPath.get(d.repoPath)?.drafts.push(d);
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

/** What the "+" button leads to. It is closed only when the environment can
 *  run none of them — each row inside says for itself whether it can. */
const ADD_PROJECT_GATES = ["openProject", "cloneProject", "createProject"] as const;

export function Sidebar() {
  const addProjectGate = useAnyGate(ADD_PROJECT_GATES);
  const workspace = useAppStore((s) => s.workspace);
  const drafts = useAppStore((s) => s.drafts);
  const selectedAgentId = useAppStore((s) => s.selectedAgentId);
  const activeDraftId = useAppStore((s) => s.activeDraftId);
  const selectedRunId = useAppStore((s) => s.selectedRunId);

  const [query, setQuery] = useState("");
  const [openMap, setOpenMap] = useState<OpenMap>({});
  // Toggles made while searching (see isGroupOpen). Reset with every query
  // change: the result set changes under the user anyway, and starting each
  // search with matches fully visible is what makes ⌘K → ↓ land on a row.
  const [searchOpenMap, setSearchOpenMap] = useState<OpenMap>({});
  const searching = query.trim().length > 0;
  function onQueryChange(q: string) {
    setQuery(q);
    setSearchOpenMap({});
  }
  function toggleGroup(key: string) {
    const setMap = searching ? setSearchOpenMap : setOpenMap;
    const fallback = searching; // isGroupOpen's default for the active map
    setMap((m) => ({ ...m, [key]: !(m[key] ?? fallback) }));
  }
  // The popover's open flag lives in the store (⌘O opens it from anywhere);
  // the modal it leads to is this component's own.
  const npOpen = useAppStore((s) => s.addProjectOpen);
  const setNpOpen = useAppStore((s) => s.setAddProjectOpen);
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

  // Projects the user is working in float to the top (see `bubbleActive`);
  // the quiet ones keep the manual order beneath them. This is what the user
  // did by hand — drag today's project up — automated, and it can be switched
  // off in Settings › Layout. Off while searching too: the result set is
  // short and its order should match the list it came from.
  //
  // Launches are on the agent records; turns are not, so each turn start is
  // stamped on its project here and kept locally. A resumed idle workspace
  // then counts as activity the moment its message is sent.
  const activeFirst = useAppStore((s) => s.features.sidebarActiveFirst);
  const turnStartedAt = useAppStore((s) => s.turnStartedAt);
  const [turns, setTurns] = useState(loadProjectActivity);
  useEffect(() => {
    const stamps: ProjectActivity = {};
    for (const g of groups) {
      for (const a of g.agents) {
        const t = turnStartedAt[a.id];
        if (t !== undefined && t > (stamps[g.key] ?? 0)) stamps[g.key] = t;
      }
    }
    // Only a newer start than the one on record is a change; otherwise this
    // would re-stamp (and re-render) on every groups update.
    setTurns((prev) => {
      const fresh = Object.entries(stamps).some(([k, t]) => t > (prev[k] ?? 0));
      return fresh ? stampProjectActivity(stamps) : prev;
    });
  }, [groups, turnStartedAt]);
  // The age cutoffs here and in each group's fold are re-checked once a minute,
  // so a project (or a row) that ages out while the app sits open settles back
  // without waiting for the next snapshot. One clock, shared with the groups.
  const now = useMinuteClock();
  const { active, rest } = useMemo(
    () =>
      activeFirst && !searching
        ? bubbleActive(filtered, now, turns)
        : { active: [], rest: filtered },
    [filtered, searching, activeFirst, turns, now],
  );

  // Reordering is only meaningful over the full, unfiltered list, and only
  // between the groups still in manual order — a floated group is neither
  // draggable nor a drop target, since its position is computed.
  const reorderable = !searching;
  const orderedPaths = useMemo(() => groups.map((g) => g.primaryPath), [groups]);
  const restPaths = useMemo(() => new Set(rest.map((g) => g.primaryPath)), [rest]);

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
      const hovered = target?.dataset.repoPath ?? null;
      const over = hovered && restPaths.has(hovered) ? hovered : null;
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

  // ↑/↓ (Home/End) step through the visible rows and select as they go. Scoped
  // to keys fired inside the list, so the composer, chat, and dropdowns keep
  // their arrows; modifier chords pass through (Alt+↑/↓ belongs to ChatNav).
  const listRef = useRef<HTMLDivElement>(null);
  function onListKeyDown(e: React.KeyboardEvent<HTMLDivElement>) {
    if (e.altKey || e.metaKey || e.ctrlKey) return;
    const rows = visibleRows(listRef.current);
    const row = rowOf(e.target);
    const target = arrowTarget(rows, row ? rows.indexOf(row) : -1, e.key);
    if (!target) return;
    e.preventDefault();
    focusRow(target);
  }

  // ↓ in the search box enters the list at the selected row (or the top), so
  // ⌘K then arrows is the whole keyboard flow.
  function enterList() {
    const rows = visibleRows(listRef.current);
    const row = rows.find((r) => r.classList.contains("active")) ?? rows[0];
    if (row) focusRow(row);
  }

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

  function renderGroup(g: ProjectGroupData, canReorder: boolean) {
    const isOver = canReorder && overPath === g.primaryPath && dragPath !== g.primaryPath;
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
        open={isGroupOpen(g.key, searching, openMap, searchOpenMap)}
        onToggle={() => toggleGroup(g.key)}
        fold={!searching}
        now={now}
        reorderable={canReorder}
        dragging={dragPath === g.primaryPath}
        dropIndicator={isOver ? (dropAfter ? "after" : "before") : null}
        onReorderPointerDown={
          canReorder ? (e, markDragged) => startReorder(g.primaryPath, e, markDragged) : undefined
        }
      />
    );
  }

  return (
    <>
      <SidebarHeader query={query} onChange={onQueryChange} onArrowDown={enterList} />
      <div className="side-scroll" ref={listRef} onKeyDown={onListKeyDown}>
        <div className="side-section">
          {/* Open while the environment can run any of the three flows behind
              it — this Mac always can, a host by its descriptor — and closed,
              with the reason, when it can run none. A disabled button gets no
              pointer events in the WebView, so the tooltip trigger has to be
              the wrapper. */}
          <span className={addProjectGate ? "tip" : undefined} data-tip={addProjectGate}>
            <button
              className="add-proj-cta flex-center text-sm"
              onClick={() => setNpOpen(true)}
              disabled={addProjectGate !== null}
              aria-label="Add project"
            >
              <Icon name="plus" size={13} />
              <span>Add project</span>
            </button>
          </span>

          {filtered.length === 0 ? (
            <div className="empty-msg" style={{ padding: "28px 12px" }}>
              <div className="et">{query ? "No matches" : "No projects yet"}</div>
              <div>{query ? "Try a different search." : "Add a repo to get started."}</div>
            </div>
          ) : (
            <>
              {active.map((g) => renderGroup(g, false))}
              {rest.map((g) => renderGroup(g, reorderable))}
            </>
          )}
        </div>
      </div>

      <SidebarFooter />

      {/* Gated like the button above: the shortcut path has no disabled state
          to stop at, so an environment that can add nothing shows nothing. */}
      {npOpen && addProjectGate === null && (
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
