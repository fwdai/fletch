import { useRef } from "react";
import type { AgentRecord, WfRun } from "@/api";
import { Icon } from "@/components/Icon";
import type { DraftAgent } from "@/store";
import { useAppStore } from "@/store";
import { RunRow } from "@/workflows/run/RunRow";
import { AgentRow } from "./AgentRow";

interface Props {
  /** Display name (basename of repo path). */
  label: string;
  /** Full repo path — used as the group's stable id. */
  repoPath: string;
  agents: AgentRecord[];
  drafts: DraftAgent[];
  /** Workflow runs grouped under this repo. */
  runs: WfRun[];
  /** Whether the user has expanded this group. */
  open: boolean;
  onToggle: () => void;
  /** Whether this group can be dragged to reorder (disabled while searching). */
  reorderable: boolean;
  /** This group is the one currently being dragged. */
  dragging: boolean;
  /** Show a drop line above ("before") or below ("after") this group, or none. */
  dropIndicator: "before" | "after" | null;
  /** Start a pointer-driven reorder from this group's header. `markDragged` is
   *  called once the pointer clears the drag threshold, so the group can swallow
   *  the trailing click. Undefined when reordering is disabled (searching). */
  onReorderPointerDown?: (e: React.PointerEvent, markDragged: () => void) => void;
}

export function ProjectGroup({
  label,
  repoPath,
  agents,
  drafts,
  runs,
  open,
  onToggle,
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
  const createDraft = useAppStore((s) => s.createDraft);
  const openProjectSettings = useAppStore((s) => s.openProjectSettings);

  const count = agents.length + drafts.length + runs.length;

  function onAddAgent(e: React.MouseEvent) {
    e.stopPropagation();
    createDraft(repoPath);
  }

  function onOpenSettings(e: React.MouseEvent) {
    e.stopPropagation();
    openProjectSettings(repoPath);
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
        title={repoPath}
      >
        <Icon name="chevR" size={10} className="chev" />
        <span className="pname">{label}</span>
        <span className="pcount">{count}</span>
        <button
          className="padd padd-settings tip"
          data-tip="Project settings"
          data-tip-down=""
          onClick={onOpenSettings}
          aria-label="Project settings"
        >
          <Icon name="settings" size={14} />
        </button>
        <button
          className="padd tip"
          data-tip="New agent  ⌘N"
          data-tip-down=""
          onClick={onAddAgent}
          aria-label="New agent"
        >
          <Icon name="plus" size={11} />
        </button>
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
        {agents.map((a) => (
          <AgentRow
            key={a.id}
            kind="real"
            agent={a}
            active={a.id === selectedAgentId}
            onClick={() => selectAgent(a.id)}
          />
        ))}
        {runs
          .filter((run) => !run.parent_run_id)
          .flatMap((run) => [
            <RunRow
              key={run.id}
              run={run}
              selected={selectedRunId === run.id}
              onSelect={() => selectRun(run.id)}
            />,
            // Composed sub-runs (§10.3) render nested under their parent.
            ...runs
              .filter((sub) => sub.parent_run_id === run.id)
              .map((sub) => (
                <RunRow
                  key={sub.id}
                  run={sub}
                  nested
                  selected={selectedRunId === sub.id}
                  onSelect={() => selectRun(sub.id)}
                />
              )),
          ])}
      </div>
    </div>
  );
}
