import { useMemo, useRef, useState } from "react";
import type { AgentRecord, WfRun } from "@/api";
import { Icon } from "@/components/Icon";
import { ProjectIcon } from "@/components/ProjectIcon";
import type { DraftAgent } from "@/store";
import { useAppStore } from "@/store";
import { useGate } from "@/store/capabilities";
import { RunRow } from "@/workflows/run/RunRow";
import { AgentRow } from "./AgentRow";
import { foldAgents } from "./foldAgents";
import { ProjectAddButton } from "./ProjectAddButton";

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
  /** Whether the user has expanded this group. */
  open: boolean;
  onToggle: () => void;
  /** Fold stale and over-cap workspaces behind an "older" row (see
   *  `foldAgents`). Off while searching: every remaining row is a match. */
  fold: boolean;
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
  open,
  onToggle,
  fold,
  now,
  reorderable,
  dragging,
  dropIndicator,
  onReorderPointerDown,
}: Props) {
  const selectedAgentId = useAppStore((s) => s.selectedAgentId);
  const activeDraftId = useAppStore((s) => s.activeDraftId);
  const selectedRunId = useAppStore((s) => s.selectedRunId);
  const selectAgent = useAppStore((s) => s.selectAgent);
  const selectDraft = useAppStore((s) => s.selectDraft);
  const selectRun = useAppStore((s) => s.selectRun);
  const openProjectScreen = useAppStore((s) => s.openProjectScreen);
  const project = useAppStore((s) => s.workspace?.projects.find((p) => p.path === repoPath));
  const roadmapGate = useGate("roadmap");

  const count = agents.length + drafts.length + runs.length;

  // "Show N older" unfolds the group until it is folded again; the choice is
  // per group and per mount, so a fresh launch starts tidy. The rows above the
  // fold keep their places either way: the older ones are revealed BELOW the
  // fold row, never re-interleaved by date, so a pinned (selected / running)
  // row that predates some hidden ones doesn't watch them jump in above it.
  const [unfolded, setUnfolded] = useState(false);
  const { shown, hidden } = useMemo(
    () =>
      fold
        ? foldAgents(agents, { now, selectedId: selectedAgentId })
        : { shown: agents, hidden: [] },
    [fold, agents, selectedAgentId, now],
  );
  const foldable = hidden.length > 0;

  function onOpenProject(e: React.MouseEvent) {
    e.stopPropagation();
    // On the tab it was last left on; ⌘⇧, and the palette's "Project
    // settings" are the ways straight to Settings.
    openProjectScreen(repoPath);
  }

  const dropClass = dropIndicator ? `drop-${dropIndicator}` : "";

  // A pointer drag that ends on the header still dispatches a trailing `click`,
  // which would toggle the group open/closed after a reorder. Track whether the
  // current interaction turned into a drag and swallow that phantom click.
  const draggedRef = useRef(false);

  return (
    <div className={`proj ${dragging ? "dragging" : ""} ${dropClass}`} data-repo-path={repoPath}>
      <div
        className={`proj-h flex-center ${open ? "open" : ""} ${reorderable ? "reorderable" : ""}`}
        onPointerDown={(e) => {
          // Left button only, and never from an action button.
          if (!onReorderPointerDown || e.button !== 0) return;
          if ((e.target as HTMLElement).closest("button")) return;
          draggedRef.current = false;
          onReorderPointerDown(e, () => {
            draggedRef.current = true;
          });
        }}
        onClick={() => {
          if (draggedRef.current) {
            draggedRef.current = false;
            return;
          }
          onToggle();
        }}
        title={repoPaths.join("\n")}
      >
        <Icon name="chevR" size={10} className="chev" />
        <ProjectIcon project={project ?? { name: label }} size={14} />
        <span className="pname">{label}</span>
        <span className="pcount">{count}</span>
        {/* The project page opens on the roadmap, so it is offered wherever
            the board is; its settings sections gate themselves on the host's
            project-settings ops (docs/remote-protocol.md, "Settings"). */}
        {!roadmapGate && (
          <button
            className="padd padd-open tip"
            data-tip="Open project"
            onClick={onOpenProject}
            aria-label="Open project"
          >
            <Icon name="map" size={14} />
          </button>
        )}
        <ProjectAddButton repoPath={repoPath} />
      </div>

      <div className={`agents ${open ? "" : "closed"}`}>
        {drafts.map((d) => (
          <AgentRow
            key={d.id}
            kind="draft"
            draft={d}
            active={d.id === activeDraftId}
            onClick={() => selectDraft(d.id)}
          />
        ))}
        {shown.map((a) => (
          <AgentRow
            key={a.id}
            kind="real"
            agent={a}
            active={a.id === selectedAgentId}
            onClick={() => selectAgent(a.id)}
          />
        ))}
        {unfolded &&
          hidden.map((a) => (
            <AgentRow
              key={a.id}
              kind="real"
              agent={a}
              active={a.id === selectedAgentId}
              onClick={() => selectAgent(a.id)}
            />
          ))}
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
        {runs.map((run) => (
          <RunRow
            key={run.id}
            run={run}
            selected={selectedRunId === run.id}
            onSelect={() => selectRun(run.id)}
          />
        ))}
      </div>
    </div>
  );
}
