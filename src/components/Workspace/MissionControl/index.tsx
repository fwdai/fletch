// MissionControl — Home as the fleet review queue (PR 3). When no agent is
// selected, the center pane answers "what needs me?": one ordered card per
// "needs you" signal across every agent and workflow run, most-decidable-first.
// The derivation is a pure selector (queue.ts); this file is the pane shell,
// keyboard triage, and the workflow review modal host.

import { useState } from "react";
import { Icon } from "@/components/Icon";
import { IconButton } from "@/components/ui/IconButton";
import { useAppStore } from "@/store";
import { AllClear } from "./AllClear";
import { FanoutCard } from "./FanoutCard";
import { QueueCard } from "./QueueCard";
import { useQueueActions } from "./useQueueActions";
import { useQueueKeyboard } from "./useQueueKeyboard";
import { useReviewQueue } from "./useReviewQueue";
import { WorkflowReviewModal } from "./WorkflowReviewModal";

export function MissionControl() {
  const leftCollapsed = useAppStore((s) => s.leftCollapsed);
  const toggleLeft = useAppStore((s) => s.toggleLeft);
  const agentCount = useAppStore((s) => s.workspace?.agents.length ?? 0);

  const items = useReviewQueue();
  const [reviewRunId, setReviewRunId] = useState<string | null>(null);
  const actions = useQueueActions(setReviewRunId);
  // Triage keys pause while the review modal is open.
  const { index, setIndex } = useQueueKeyboard({
    items,
    active: reviewRunId === null,
    onEnter: actions.enter,
    onApprove: actions.approve,
    onRequestChanges: actions.requestChanges,
  });

  return (
    <div className="pane center fade-in">
      <div className="center-h flex-center">
        <IconButton
          tip={leftCollapsed ? "Show sidebar (⌘B)" : "Hide sidebar (⌘B)"}
          onClick={toggleLeft}
        >
          <Icon name="sidebarL" />
        </IconButton>
        <div className="task">
          <div className="t-name">
            <Icon name="layers" size={14} style={{ color: "var(--accent)", flexShrink: 0 }} />
            <span>Mission Control</span>
          </div>
          <div className="t-meta">
            {items.length === 0
              ? "review queue"
              : `${items.length} ${items.length === 1 ? "item" : "items"} need you`}
          </div>
        </div>
      </div>

      <div className="mc-scroll">
        {items.length === 0 ? (
          <AllClear hasAgents={agentCount > 0} />
        ) : (
          <div className="mc-wrap">
            <div className="mc-list">
              {items.map((item, i) =>
                item.kind === "fanout" ? (
                  <FanoutCard
                    key={item.id}
                    item={item}
                    focused={i === index}
                    onFocus={() => setIndex(i)}
                    onEnter={() => actions.enter(item)}
                    onUpdateAll={() => actions.approve(item)}
                    onDismiss={() => actions.dismiss(item)}
                  />
                ) : (
                  <QueueCard
                    key={item.id}
                    item={item}
                    focused={i === index}
                    onFocus={() => setIndex(i)}
                    onEnter={() => actions.enter(item)}
                    onApprove={() => actions.approve(item)}
                    onRequestChanges={() => actions.requestChanges(item)}
                    onDismiss={() => actions.dismiss(item)}
                  />
                ),
              )}
            </div>
            <div className="mc-hints" aria-hidden="true">
              <span className="mc-hint">
                <kbd>j</kbd>
                <kbd>k</kbd> navigate
              </span>
              <span className="mc-hint">
                <kbd>↵</kbd> review
              </span>
              <span className="mc-hint">
                <kbd>a</kbd> approve
              </span>
              <span className="mc-hint">
                <kbd>r</kbd> request changes
              </span>
            </div>
          </div>
        )}
      </div>

      {reviewRunId && (
        <WorkflowReviewModal runId={reviewRunId} onClose={() => setReviewRunId(null)} />
      )}
    </div>
  );
}
