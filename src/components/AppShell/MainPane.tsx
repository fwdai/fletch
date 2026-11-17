import { useEffect } from "react";
import { ErrorBoundary } from "@/components/ErrorBoundary";
import { RightPanel } from "@/components/RightPanel";
import { Workspace } from "@/components/Workspace";
import { useAppStore, useHostId } from "@/store";
import { useFocusedGitSync } from "@/store/gitSync";
import { useSplitter } from "@/util/splitter";
import { RunActivityPanel } from "@/workflows/run/ActivityPanel";

/** The center pane and the right rail beside it, in the scope of the host on
 *  screen: what that host has selected is what they show. */
export function MainPane() {
  // Part of every remount key below: agent ids repeat across hosts, and two
  // same-named agents are not the same workspace (docs/multi-host-plan.md
  // §1.3).
  const hostId = useHostId();
  const leftCollapsed = useAppStore((s) => s.leftCollapsed);
  const rightCollapsed = useAppStore((s) => s.rightCollapsed);
  const leftWidth = useAppStore((s) => s.leftWidth);
  const rightWidth = useAppStore((s) => s.rightWidth);
  const setRightWidth = useAppStore((s) => s.setRightWidth);
  const commitRightWidth = useAppStore((s) => s.commitRightWidth);
  const activeDraftId = useAppStore((s) => s.activeDraftId);
  const selectedAgentId = useAppStore((s) => s.selectedAgentId);
  const selectedRunId = useAppStore((s) => s.selectedRunId);
  const workspace = useAppStore((s) => s.workspace);
  const markResultsSeen = useAppStore((s) => s.markResultsSeen);

  // A host coming back on screen shows the agent it had selected, so whatever
  // that agent finished meanwhile is seen. `markResultsSeen` is the scope's
  // own, so this runs for the host that just came on screen.
  useEffect(() => {
    if (selectedAgentId) markResultsSeen(selectedAgentId);
  }, [selectedAgentId, markResultsSeen]);

  useFocusedGitSync();
  const onRightDrag = useSplitter(rightWidth, setRightWidth, "right", commitRightWidth);

  const selectedAgent = workspace?.agents.find((a) => a.id === selectedAgentId);
  // The right rail serves two masters: an agent's Code/Git/Run/Terminal panels,
  // and a workflow run's Activity panel. Same pane, same splitter, same ⌘].
  const rightPaneContent = selectedAgent || selectedRunId;
  const rightPaneVisible = !rightCollapsed && !activeDraftId && rightPaneContent;

  return (
    <>
      {/* Keyed by agent so switching agents clears a stuck error — and by
          host, because agent ids repeat across hosts and two same-named
          agents are not the same workspace. */}
      <ErrorBoundary label="the workspace" key={`${hostId}:${selectedAgentId}`}>
        <Workspace />
      </ErrorBoundary>

      {rightPaneVisible && <div className="splitter" onMouseDown={onRightDrag} />}
      {/* Only mount the right pane when something is selected that has
       *  panel content — an agent (Code/Git/Run/Terminal) or a workflow
       *  run (Activity). Without this gate the container still claims
       *  layout width on Home, stranding the center pane in a narrow
       *  column beside an empty rail. */}
      {!activeDraftId && rightPaneContent && (
        <div
          className={`pane right ${rightCollapsed ? "collapsed" : ""}`}
          style={{
            // Default to the stored width, but never wider than a 50:50
            // split with the center pane. `100%` resolves against `.main`,
            // so subtracting the left pane leaves the center+right region;
            // half of that is the even-split cap. Window/left resizes
            // recompute it automatically (no measurement needed).
            width: rightCollapsed
              ? 0
              : `min(${rightWidth}px, calc((100% - ${leftCollapsed ? 0 : leftWidth}px) / 2))`,
          }}
        >
          {!rightCollapsed && selectedAgent ? (
            <ErrorBoundary label="the side panel" key={`${hostId}:${selectedAgent.id}`}>
              <RightPanel agent={selectedAgent} />
            </ErrorBoundary>
          ) : !rightCollapsed && selectedRunId ? (
            <ErrorBoundary label="the side panel" key={selectedRunId}>
              <RunActivityPanel runId={selectedRunId} />
            </ErrorBoundary>
          ) : null}
        </div>
      )}
    </>
  );
}
