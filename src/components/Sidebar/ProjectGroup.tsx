import { useMemo, useRef, useState } from "react";
import type { AgentRecord, WfRun } from "@/api";
import { HostChip } from "@/components/HostChip";
import { Icon } from "@/components/Icon";
import { ProjectIcon } from "@/components/ProjectIcon";
import type { DraftAgent } from "@/store";
import { onHost, useAppStore, useHostId, useOnHost, useOnScreenSelection } from "@/store";
import { useGate } from "@/store/capabilities";
import { EngineScope } from "@/store/EngineScope";
import { type EnvironmentId, LOCAL_ENVIRONMENT_ID } from "@/store/environments";
import { useShortcutKeys } from "@/util/shortcuts";
import { RunRow } from "@/workflows/run/RunRow";
import { AgentRow } from "./AgentRow";
import { foldAgents } from "./foldAgents";
import { rowHost, type SidebarRow } from "./hostGroups";

interface Props {
  /** Project display name. */
  label: string;
  /** The project's primary repo path — the drag handle id and the target for
   *  new drafts and the settings modal. */
  repoPath: string;
  /** All repos attached to the project (equals [repoPath] for single-repo
   *  projects) — shown in the header tooltip. */
  repoPaths: string[];
  agents: AgentRecord[];
  drafts: DraftAgent[];
  /** Workflow runs grouped under this repo. */
  runs: WfRun[];
  /** The host each row runs on (`ProjectGroupData.hostOf`). Every row is
   *  rendered in its host's scope, so it reads and acts on that host. */
  hostOf: ReadonlyMap<SidebarRow, EnvironmentId>;
  /** Whether the user has expanded this group. */
  open: boolean;
  onToggle: () => void;
  /** Fold stale and over-cap workspaces behind an "older" row (see
   *  `foldAgents`). Off while searching: every remaining row is a match. */
  fold: boolean;
  /** When a turn last started in each workspace, by agent id: what the fold
   *  counts as recent, beside the launch time. */
  activity: Readonly<Record<string, number>>;
  /** The sidebar's minute clock, so the fold's age cutoff moves with time. */
  now: number;
  /** Whether this group can be dragged to reorder: only with "Active projects
   *  first" off, and not while searching. */
  reorderable: boolean;
  /** This group is the one currently being dragged. */
  dragging: boolean;
  /** Show a drop line above ("before") or below ("after") this group, or none. */
  dropIndicator: "before" | "after" | null;
  /** Start a pointer-driven reorder from this group's header. `markDragged` is
   *  called once the pointer clears the drag threshold, so the group can swallow
   *  the trailing click. Undefined when the group isn't `reorderable`. */
  onReorderPointerDown?: (e: React.PointerEvent, markDragged: () => void) => void;
}

export function ProjectGroup({
  label,
  repoPath,
  repoPaths,
  agents,
  drafts,
  runs,
  hostOf,
  open,
  onToggle,
  fold,
  activity,
  now,
  reorderable,
  dragging,
  dropIndicator,
  onReorderPointerDown,
}: Props) {
  // The group's header is its project's on the host it is scoped to: This
  // Mac for every project it has, a paired host for one only that host has.
  // Its actions put that host on screen before they open anything there.
  const groupHost = useHostId();
  const onGroupHost = useOnHost();
  const newAgentKeys = useShortcutKeys("newAgent");
  const project = useAppStore((s) => s.workspace?.projects.find((p) => p.path === repoPath));
  const roadmapGate = useGate("roadmap");

  // A row is the selection only on the host on screen: ids repeat across
  // hosts, and every host keeps its own selection.
  const selection = useOnScreenSelection();
  const hostOfRow = (row: SidebarRow) => rowHost({ hostOf }, row);
  const isOnScreen = (row: SidebarRow, id: string | null) =>
    id !== null && row.id === id && hostOfRow(row) === selection.hostId;
  const selectedAgent = agents.find((a) => isOnScreen(a, selection.agentId));

  const count = agents.length + drafts.length + runs.length;
  // A project with nothing in it has nothing to fold: it draws as one quiet
  // line (see `.proj.empty`).
  const empty = count === 0;
  // The group the selection is in keeps its "+" on show; the others reveal
  // theirs on hover.
  const current =
    selectedAgent !== undefined ||
    drafts.some((d) => isOnScreen(d, selection.draftId)) ||
    runs.some((r) => isOnScreen(r, selection.runId));

  // "Show N older" unfolds the group until it is folded again; the choice is
  // per group and per mount, so a fresh launch starts tidy. The rows above the
  // fold keep their places either way: the older ones are revealed BELOW the
  // fold row, never re-interleaved by date, so a pinned (selected / running)
  // row that predates some hidden ones doesn't watch them jump in above it.
  const [unfolded, setUnfolded] = useState(false);
  const { shown, hidden } = useMemo(() => {
    if (!fold) return { shown: agents, hidden: [] };
    // `activity` is This Mac's turns, by agent id: it says nothing about a
    // paired host's workspace of the same id.
    const turnAt = (a: AgentRecord) =>
      rowHost({ hostOf }, a) === LOCAL_ENVIRONMENT_ID ? activity[a.id] : undefined;
    return foldAgents(agents, { now, selected: selectedAgent, turnAt });
  }, [fold, agents, selectedAgent, activity, hostOf, now]);
  const foldable = hidden.length > 0;

  const dropClass = dropIndicator ? `drop-${dropIndicator}` : "";

  // A pointer drag that ends on the header still dispatches a trailing `click`,
  // which would toggle the group open/closed after a reorder. Track whether the
  // current interaction turned into a drag and swallow that phantom click.
  const draggedRef = useRef(false);
  const dragged = () => {
    if (!draggedRef.current) return false;
    draggedRef.current = false;
    return true;
  };

  /** One of the header's actions: stops the header's own click (the fold),
   *  and does nothing after a drag. */
  const action = (open: () => void) => (e: React.MouseEvent) => {
    e.stopPropagation();
    if (!dragged()) open();
  };

  /** One agent row, in the scope of the host it runs on. */
  const agentRow = (a: AgentRecord) => {
    const hostId = hostOfRow(a);
    return (
      <EngineScope key={`${hostId}:${a.id}`} hostId={hostId}>
        <AgentRow
          kind="real"
          agent={a}
          active={isOnScreen(a, selection.agentId)}
          onClick={() => onHost(hostId, (s) => s.selectAgent(a.id))}
        />
      </EngineScope>
    );
  };

  return (
    <div
      className={[
        "proj",
        empty ? "empty" : "",
        current ? "current" : "",
        dragging ? "dragging" : "",
        dropClass,
      ]
        .filter(Boolean)
        .join(" ")}
      // Only This Mac's projects are in the manual order, so only they are
      // drop targets for a reorder.
      data-repo-path={groupHost === LOCAL_ENVIRONMENT_ID ? repoPath : undefined}
    >
      {/* The header is the project's own row. Its name opens the Roadmap — the
          project's home — and the rest of the row folds the agents, the way a
          file tree's arrow expands while its name opens. The hover action at
          the right is the project's other door, its settings. */}
      <div
        className={`proj-h flex-center ${open ? "open" : ""} ${reorderable ? "reorderable" : ""}`}
        onPointerDown={(e) => {
          // Left button only, and never from the action buttons at the right
          // (the name is a grab point like the rest of the row).
          if (!onReorderPointerDown || e.button !== 0) return;
          if ((e.target as HTMLElement).closest(".padd")) return;
          draggedRef.current = false;
          onReorderPointerDown(e, () => {
            draggedRef.current = true;
          });
        }}
        onClick={() => {
          if (!dragged()) onToggle();
        }}
        title={repoPaths.join("\n")}
      >
        <Icon name="chevR" size={10} className="chev" />
        <ProjectIcon project={project ?? { name: label }} size={14} />
        {roadmapGate ? (
          <span className="pname" title={roadmapGate}>
            {label}
          </span>
        ) : (
          <button
            type="button"
            className="pname tip"
            data-tip="Open roadmap"
            onClick={action(() => onGroupHost((s) => s.openRoadmap(repoPath)))}
          >
            {label}
          </button>
        )}
        <HostChip />
        {/* The rows say how many while they show; the count stands in for
            them once the group is folded. */}
        {!open && !empty && <span className="pcount">{count}</span>}
        {/* The settings panes gate themselves on the host's project-settings
            ops (docs/remote-protocol.md, "Settings"). */}
        <button
          type="button"
          className="padd padd-quiet tip"
          data-tip="Project settings"
          onClick={action(() => onGroupHost((s) => s.openProjectSettings(repoPath)))}
          aria-label="Project settings"
        >
          <Icon name="settings" size={14} />
        </button>
        <button
          type="button"
          className="padd padd-add tip"
          data-tip={`New agent  ${newAgentKeys}`}
          onClick={action(() => onGroupHost((s) => void s.createDraft(repoPath)))}
          aria-label="New agent"
        >
          <Icon name="plus" size={11} />
        </button>
      </div>

      <div className={`agents ${open ? "" : "closed"}`}>
        {drafts.map((d) => {
          const hostId = hostOfRow(d);
          return (
            <EngineScope key={`${hostId}:${d.id}`} hostId={hostId}>
              <AgentRow
                kind="draft"
                draft={d}
                active={isOnScreen(d, selection.draftId)}
                onClick={() => onHost(hostId, (s) => s.selectDraft(d.id))}
              />
            </EngineScope>
          );
        })}
        {shown.map(agentRow)}
        {unfolded && hidden.map(agentRow)}
        {/* The toggle closes the list either way: "N older" under the recent
            rows, "Show fewer" under the revealed ones, where the eye is. */}
        {foldable && (
          <button
            type="button"
            className="agents-fold text-sm"
            onClick={() => setUnfolded((v) => !v)}
          >
            <Icon name={unfolded ? "chevU" : "chevD"} size={10} />
            {unfolded ? "Show fewer" : `${hidden.length} older`}
          </button>
        )}
        {runs.map((run) => {
          const hostId = hostOfRow(run);
          return (
            <EngineScope key={`${hostId}:${run.id}`} hostId={hostId}>
              <RunRow
                run={run}
                selected={isOnScreen(run, selection.runId)}
                onSelect={() => onHost(hostId, (s) => s.selectRun(run.id))}
              />
            </EngineScope>
          );
        })}
      </div>
    </div>
  );
}
