import type { KeyboardEvent } from "react";
import { AgentIdentityChip } from "@/components/AgentIdentityChip";
import { Icon } from "@/components/Icon";
import { stuckLabel } from "@/helpers/autopilotCopy";
import { EvidenceChips } from "./EvidenceChips";
import type { ReviewItem, ReviewReason } from "./queue";

/** The "what" line — a short phrase per reason, joined when a card carries
 *  several (an agent with unseen results AND a failing PR). */
const REASON_LABEL: Record<Exclude<ReviewReason, "autopilot-stuck">, string> = {
  "workflow-approval": "Awaiting approval",
  "workflow-conflict": "Merge conflict",
  "unseen-results": "New results to review",
  "checks-failing": "Checks failing",
  "unresolved-comments": "Unresolved comments",
};

function reasonLabel(reason: ReviewReason, item: ReviewItem): string {
  // Autopilot gave up: say why, as a fact about the PR, in the same words the
  // Git panel uses. "Needs you" is what the whole queue already means.
  if (reason === "autopilot-stuck") {
    return item.autopilotStuck
      ? stuckLabel(item.autopilotStuck.reason, item.autopilotStuck.rung)
      : "Needs you";
  }
  return REASON_LABEL[reason];
}

function reasonLine(item: ReviewItem): string {
  return item.reasons.map((r) => reasonLabel(r, item)).join(" · ");
}

interface Props {
  item: ReviewItem;
  focused: boolean;
  onFocus: () => void;
  onEnter: () => void;
  onApprove: () => void;
  onRequestChanges: () => void;
  onDismiss: () => void;
}

/** One "needs you" card: who (identity chip) → what (reason) → evidence (chips)
 *  → action. The whole card opens the review; the action buttons are explicit
 *  affordances mirroring the a/r keys. Quiet by default; the focused card gets a
 *  subtle accent rail + tint so keyboard triage is always locatable. */
export function QueueCard({
  item,
  focused,
  onFocus,
  onEnter,
  onApprove,
  onRequestChanges,
  onDismiss,
}: Props) {
  const onKeyDown = (e: KeyboardEvent) => {
    if (e.target !== e.currentTarget) return;
    if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      onEnter();
    }
  };

  return (
    <div
      className={`mc-card ${focused ? "focused" : ""}`}
      role="button"
      tabIndex={0}
      aria-current={focused ? "true" : undefined}
      onMouseEnter={onFocus}
      onClick={onEnter}
      onKeyDown={onKeyDown}
    >
      <span className="mc-card-rail" />
      <div className="mc-card-body">
        <div className="mc-card-head">
          {item.kind === "workflow" ? (
            <span className="mc-wf-chip iflex-center" title="Workflow run">
              <Icon name="combine" size={14} />
            </span>
          ) : (
            item.agent && <AgentIdentityChip agent={item.agent} size={18} />
          )}
          <span className="mc-card-title truncate">{item.title}</span>
          <span className="mc-card-reason">{reasonLine(item)}</span>
        </div>
        <div className="mc-card-goal truncate">{item.goal}</div>
        <EvidenceChips item={item} />
      </div>
      <div className="mc-card-actions">
        <button
          type="button"
          className="mc-btn primary"
          onClick={(e) => {
            e.stopPropagation();
            onApprove();
          }}
        >
          <Icon name="check" size={12} /> Approve
        </button>
        <button
          type="button"
          className="mc-btn"
          onClick={(e) => {
            e.stopPropagation();
            onRequestChanges();
          }}
        >
          <Icon name="arrowR" size={12} /> Request changes
        </button>
        <button
          type="button"
          className="mc-dismiss tip"
          data-tip="Dismiss until this changes"
          aria-label="Dismiss"
          onClick={(e) => {
            e.stopPropagation();
            onDismiss();
          }}
        >
          <Icon name="close" size={12} />
        </button>
      </div>
    </div>
  );
}
