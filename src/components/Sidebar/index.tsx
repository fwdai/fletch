import { useEffect, useMemo, useRef, useState } from "react";
import { NewProject, type NewProjectMode } from "@/components/NewProject";
import { useAppStore, useEveryHost, useOnScreenSelection } from "@/store";
import { EngineScope } from "@/store/EngineScope";
import { type EnvironmentId, LOCAL_ENVIRONMENT_ID } from "@/store/environments";
import { arrowTarget } from "@/util/arrowNav";
import { basename, plural } from "@/util/format";
import { useMinuteClock } from "@/util/hooks";
import { isGroupOpen, type OpenMap } from "./groupOpen";
import { groupSignal, type ProjectGroupData, rowHost, type SidebarRow } from "./hostGroups";
import { NewProjectPopover } from "./NewProjectPopover";
import { PaletteHint } from "./PaletteHint";
import { ProjectGroup } from "./ProjectGroup";
import type { ProjectOption } from "./ProjectSelector";
import { applyFocus } from "./projectFocus";
import type { SignalSources } from "./projectSignal";
import { bubbleActive } from "./recentProjects";
import { focusRow, SIDEBAR_LIST_ID, stepOrigin, visibleRows } from "./rowNav";
import { SidebarFooter } from "./SidebarFooter";
import { SidebarHeader } from "./SidebarHeader";
import { useHostGroups } from "./useHostGroups";
import { useHostRuns } from "./useHostRuns";
import { useProjectFocus } from "./useProjectFocus";
import { useProjectReorder } from "./useProjectReorder";
import { useTurnActivity } from "./useTurnActivity";

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
          a.title?.toLowerCase().includes(needle) ||
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

/** The project list, over every host: This Mac's projects with the work of
 *  every paired host that runs in one of them, then the projects only a
 *  paired host has (see `groupByProject`). Rendered in This Mac's scope, with
 *  each row (and a paired host's group) in the scope of the host it is on. */
export function Sidebar() {
  const workspace = useAppStore((s) => s.workspace);
  // What the main pane shows: one selection for the window, on one host.
  const selection = useOnScreenSelection();

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
  // The popover's open flag lives in the store (⌘O opens it from anywhere,
  // and the store applies the gate); the modal it leads to is this
  // component's own.
  const npOpen = useAppStore((s) => s.addProjectOpen);
  const closeAddProject = useAppStore((s) => s.closeAddProject);
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
  const { runsByHost, feeds } = useHostRuns();
  const groups = useHostGroups(runsByHost, sortPaths);

  // The project selector narrows the list to a focus; the search then filters
  // inside it. Its menu lists every project, in the manual order.
  const groupKeys = useMemo(() => groups.map((g) => g.key), [groups]);
  const { focus, setFocus } = useProjectFocus(groupKeys);
  // Each host's maps are selected whole (stable references) and read outside
  // the selector, as the agent rows do.
  const hostIds = useEveryHost((_s, hostId) => hostId);
  const pendingToolUse = useEveryHost((s) => s.pendingToolUse);
  const unseenResults = useEveryHost((s) => s.unseenResults);
  const projectOptions = useMemo<ProjectOption[]>(() => {
    const sourcesOf = (hostId: EnvironmentId): SignalSources => {
      const i = hostIds.indexOf(hostId);
      return { pending: pendingToolUse[i] ?? {}, unseen: unseenResults[i] ?? {} };
    };
    return groups.map((g) => ({
      key: g.key,
      label: g.label,
      // A project only a paired host has shows its initial: the icon is that
      // host's to serve.
      icon: (g.hostId === LOCAL_ENVIRONMENT_ID &&
        workspace?.projects.find((p) => p.path === g.primaryPath)) || { name: g.label },
      count: g.agents.length + g.drafts.length + g.runs.length,
      signal: groupSignal(g, sourcesOf),
    }));
  }, [groups, workspace?.projects, hostIds, pendingToolUse, unseenResults]);
  // Focusing on a project is asking to see its workspaces, so the projects
  // picked open with the pick — wherever it was made: the selector, the
  // palette or ⌘⇧P. Keyed on the store's own array, which only changes when
  // a focus is set, so a group the user then closes stays closed.
  const savedFocus = useAppStore((s) => s.projectFocus);
  useEffect(() => {
    setOpenMap((m) => ({ ...m, ...Object.fromEntries(savedFocus.map((k) => [k, true])) }));
  }, [savedFocus]);
  const filtered = useMemo(
    () => applySearch(applyFocus(groups, focus), query),
    [groups, focus, query],
  );
  // An empty list is the filter's doing only while there are projects to
  // filter. With none, the header has no filter field (see `SidebarHeader`),
  // so text left in it — typed before the last project went, or in another
  // environment — must not be blamed for the empty list.
  const unmatched = query.trim().length > 0 && groups.length > 0;
  // What the text matches in the projects the focus leaves out. A search that
  // comes back short, or empty, because of the focus says so and offers the
  // way to them, rather than reading as "there is no such workspace".
  const beyondFocus = useMemo(() => {
    if (!query.trim() || focus.length === 0) return null;
    const shown = new Set(focus);
    const hidden = applySearch(
      groups.filter((g) => !shown.has(g.key)),
      query,
    );
    if (hidden.length === 0) return null;
    const rows = hidden.reduce((n, g) => n + g.agents.length + g.drafts.length + g.runs.length, 0);
    return rows > 0
      ? `${plural(rows, "more match", "more matches")} in other projects`
      : `${plural(hidden.length, "other project matches", "other projects match")}`;
  }, [groups, focus, query]);

  // Projects the user is working in float to the top (see `bubbleActive`);
  // the quiet ones keep the manual order beneath them. This is what the user
  // did by hand — drag today's project up — automated, and it can be switched
  // off in Settings › Layout. Off while searching too: the result set is
  // short and its order should match the list it came from.
  const activeFirst = useAppStore((s) => s.features.sidebarActiveFirst);
  // Turns are stamped for This Mac's workspaces: the stamps are kept by agent
  // id, which a paired host's agents share with this Mac's.
  const localWork = useMemo(
    () =>
      groups.map((g) => ({
        key: g.key,
        agents: g.agents.filter((a) => rowHost(g, a) === LOCAL_ENVIRONMENT_ID),
      })),
    [groups],
  );
  const { projectTurns, agentTurns } = useTurnActivity(localWork);
  // The age cutoffs here and in each group's fold are re-checked once a minute,
  // so a project (or a row) that ages out while the app sits open settles back
  // without waiting for the next snapshot. One clock, shared with the groups.
  const now = useMinuteClock();
  const { active, rest } = useMemo(
    () =>
      activeFirst && !searching
        ? bubbleActive(filtered, now, projectTurns)
        : { active: [], rest: filtered },
    [filtered, searching, activeFirst, projectTurns, now],
  );

  // An empty project has no workspace to scan for, so in the computed order
  // the empty ones settle at the tail, where they stack as one compact block
  // (see `.proj.empty`). The manual order, and a search's, stay as they are.
  // Whatever the order, the projects only a paired host has come after This
  // Mac's.
  const ordered = useMemo(() => {
    const isMine = (g: ProjectGroupData) => g.hostId === LOCAL_ENVIRONMENT_ID;
    const isEmpty = (g: ProjectGroupData) =>
      g.agents.length + g.drafts.length + g.runs.length === 0;
    let list = [...active, ...rest];
    if (activeFirst && !searching) {
      list = [...list.filter((g) => !isEmpty(g)), ...list.filter(isEmpty)];
    }
    return [...list.filter(isMine), ...list.filter((g) => !isMine(g))];
  }, [active, rest, activeFirst, searching]);

  // Dragging is the manual mode. With active-first on the top of the list is
  // computed, so nothing drags, and the quiet tail keeps the order last saved.
  // Off while searching too: a reorder only means something over the full,
  // unfiltered list.
  const reorderable = !activeFirst && !searching;
  const orderedPaths = useMemo(
    () => groups.filter((g) => g.hostId === LOCAL_ENVIRONMENT_ID).map((g) => g.primaryPath),
    [groups],
  );

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

  // The add-project popover's flag outlives this component (it is in the
  // store so ⌘O can raise it); drop it on unmount so a popover left open when
  // a full-screen surface took over doesn't reappear when that surface closes.
  useEffect(() => () => closeAddProject(), [closeAddProject]);

  // ↑/↓ (Home/End) step through the visible rows and select as they go. Scoped
  // to keys fired inside the list, so the composer, chat, and dropdowns keep
  // their arrows; modifier chords pass through (Alt+↑/↓ belongs to ChatNav).
  const listRef = useRef<HTMLDivElement>(null);
  function onListKeyDown(e: React.KeyboardEvent<HTMLDivElement>) {
    if (e.altKey || e.metaKey || e.ctrlKey) return;
    const rows = visibleRows(listRef.current);
    const target = arrowTarget(rows, stepOrigin(rows, e.target, e.key), e.key);
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
  const { hostId: onScreenHost, agentId, draftId, runId } = selection;
  useEffect(() => {
    setOpenMap((prev) => {
      const next = { ...prev };
      const selected = (g: ProjectGroupData, row: SidebarRow, id: string | null) =>
        row.id === id && rowHost(g, row) === onScreenHost;
      for (const g of groups) {
        if (g.agents.some((a) => selected(g, a, agentId))) next[g.key] = true;
        if (g.drafts.some((d) => selected(g, d, draftId))) next[g.key] = true;
        if (g.runs.some((r) => selected(g, r, runId))) next[g.key] = true;
        if (!(g.key in next)) {
          next[g.key] = g.agents.length > 0 || g.drafts.length > 0 || g.runs.length > 0;
        }
      }
      return next;
    });
  }, [groups, onScreenHost, agentId, draftId, runId]);

  function renderGroup(g: ProjectGroupData) {
    // Only This Mac's projects are in the manual order.
    const canReorder = reorderable && g.hostId === LOCAL_ENVIRONMENT_ID;
    const isOver = canReorder && overPath === g.primaryPath && dragPath !== g.primaryPath;
    const dropAfter =
      isOver &&
      dragPath != null &&
      orderedPaths.indexOf(dragPath) < orderedPaths.indexOf(g.primaryPath);
    return (
      <EngineScope key={g.key} hostId={g.hostId}>
        <ProjectGroup
          label={g.label}
          repoPath={g.primaryPath}
          repoPaths={g.repoPaths}
          agents={g.agents}
          drafts={g.drafts}
          runs={g.runs}
          hostOf={g.hostOf}
          open={isGroupOpen(g.key, searching, openMap, searchOpenMap)}
          onToggle={() => toggleGroup(g.key)}
          fold={!searching}
          activity={agentTurns}
          now={now}
          reorderable={canReorder}
          dragging={canReorder && dragPath === g.primaryPath}
          dropIndicator={isOver ? (dropAfter ? "after" : "before") : null}
          onReorderPointerDown={
            canReorder ? (e, markDragged) => startReorder(g.primaryPath, e, markDragged) : undefined
          }
        />
      </EngineScope>
    );
  }

  return (
    <>
      <SidebarHeader
        projects={projectOptions}
        focus={focus}
        onFocusChange={setFocus}
        query={query}
        onQueryChange={onQueryChange}
        onArrowDown={enterList}
      />
      <div id={SIDEBAR_LIST_ID} className="side-scroll" ref={listRef} onKeyDown={onListKeyDown}>
        <div className="side-section">
          {filtered.length === 0 ? (
            <div className="empty-msg" style={{ padding: "28px 12px" }}>
              <div className="et">{unmatched ? "No matches" : "No projects yet"}</div>
              <div>
                {!unmatched
                  ? "Add a repo to get started."
                  : beyondFocus
                    ? "Nothing in the projects shown."
                    : "Try a different search."}
              </div>
            </div>
          ) : (
            ordered.map((g) => renderGroup(g))
          )}
          {beyondFocus && (
            <button type="button" className="side-beyond text-sm" onClick={() => setFocus([])}>
              {beyondFocus} · Show all projects
            </button>
          )}
        </div>
      </div>

      <PaletteHint />
      <SidebarFooter />
      {feeds}

      {npOpen && (
        <NewProjectPopover
          onClose={closeAddProject}
          onChoose={(mode) => {
            closeAddProject();
            setNpMode(mode);
          }}
        />
      )}
      {npMode && <NewProject mode={npMode} onClose={() => setNpMode(null)} />}
    </>
  );
}
