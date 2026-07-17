import type { KeyboardEvent } from "react";
import { Icon } from "@/components/Icon";
import type { ReviewItem } from "./queue";

interface Props {
  item: ReviewItem;
  focused: boolean;
  onFocus: () => void;
  /** ↵ — open the merged PR on GitHub. */
  onEnter: () => void;
  /** a — Update all: delegate `update-branch` to every affected agent. */
  onUpdateAll: () => void;
  onDismiss: () => void;
}

/** A merge fan-out card (§3): one clear sentence — a sibling's PR merged and N
 *  agents on the repo are now behind — with one obvious action, "Update all",
 *  which flips each affected agent into its delegated/running state through the
 *  existing update-branch machinery. The affected agents are listed as quiet
 *  chips so the scope is visible without extra UI. Shares the queue-card shell
 *  so it reads as one system with the agent/workflow cards. */
export function FanoutCard({ item, focused, onFocus, onEnter, onUpdateAll, onDismiss }: Props) {
  const fanout = item.fanout;
  if (!fanout) return null;

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
          <span className="mc-wf-chip iflex-center" title="Base moved">
            <Icon name="merge" size={14} />
          </span>
          <span className="mc-card-title truncate">{fanout.merged.title} merged</span>
          <span className="mc-card-reason">base moved</span>
        </div>
        <div className="mc-card-goal truncate">{item.goal}</div>
        <div className="mc-chips">
          {fanout.agents.map((a) => (
            <span key={a.agentId} className="mc-chip mc-chip-muted" title={`${a.behind} behind`}>
              <Icon name="branch" size={11} />
              {a.name}
            </span>
          ))}
        </div>
      </div>
      <div className="mc-card-actions">
        <button
          type="button"
          className="mc-btn primary"
          onClick={(e) => {
            e.stopPropagation();
            onUpdateAll();
          }}
        >
          <Icon name="loop" size={12} /> Update all
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
