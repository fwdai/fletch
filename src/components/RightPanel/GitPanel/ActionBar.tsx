import type { ReactNode } from "react";
import { Icon } from "@/components/Icon";
import type { ActionTone, GitPanelState, StatusKind } from "@/components/RightPanel/primaryActions";
import { SplitAction, type SplitActionItem } from "./SplitAction";
import { GitLink, Spinner } from "./shared";

/** States whose idle status would only repeat the header: the pill and its
 *  text already say "PR #7 · checks failing", "#7 · closed", "#7 → main". The
 *  slot stays empty and the action button stands alone. */
const HEADER_SAYS_IT: readonly GitPanelState[] = ["pr-open", "pr-closed", "merged"];

/** The pinned footer's action row: a single status slot (busy spinner →
 *  delegation → transient notice → the idle status) followed by the split
 *  action button. The idle status says only what the header doesn't. */
export function ActionBar({
  statusKind,
  statusLabel,
  idle,
  busy,
  delegationLabel,
  autoAttempt,
  notice,
  panelState,
  pushedLink,
  aheadCount,
  items,
  selectedKey,
  tone,
  mainDisabled,
  mainReason,
  onSelect,
  onRun,
}: {
  statusKind: StatusKind;
  statusLabel: string;
  /** Replaces `statusLabel` in the idle slot when set — the changes state's
   *  "who writes the commit message" note (see `CommitStatus`). */
  idle?: ReactNode;
  busy: string | null;
  delegationLabel: string | null;
  /** Set when the in-flight delegation was started by autopilot, not a click —
   *  the retry number of that cycle. A second or third try is exactly when the
   *  user wants to know before it gives up. */
  autoAttempt: number | null;
  notice: string | null;
  panelState: GitPanelState;
  pushedLink: string | null;
  aheadCount: number;
  items: SplitActionItem[];
  selectedKey: string;
  tone: ActionTone;
  mainDisabled: boolean;
  /** Why the selected action can't run here — a capability the environment
   *  lacks, which nothing the user does in the panel will change. Shown in the
   *  status slot below the transient states, which still take precedence: what
   *  is happening right now is more urgent than what cannot happen at all. */
  mainReason?: string | null;
  onSelect: (key: string) => void;
  onRun: () => void;
}) {
  const showIdle = idle != null || !HEADER_SAYS_IT.includes(panelState);
  return (
    <div className="git-act flex-center">
      {busy ? (
        <div className="git-act-status flex-center info text-xs">
          <Spinner />
          <span className="lbl">{busy}</span>
        </div>
      ) : delegationLabel ? (
        <div className="git-act-status flex-center info working text-xs">
          <Spinner />
          {/* The one mark autopilot gets: this turn started itself. Absent on a
           *  turn the user clicked for, so the glyph always means the same thing. */}
          {autoAttempt != null && (
            <span
              className="git-auto tip"
              data-tip="Started automatically — autopilot keeps this PR mergeable"
              aria-label="Started automatically"
            >
              <Icon name="zap" size={11} />
            </span>
          )}
          <span className="lbl">
            {delegationLabel}
            {autoAttempt != null && autoAttempt > 1 && ` (try ${autoAttempt})`}
          </span>
        </div>
      ) : notice ? (
        <div className="git-notice iflex-center text-xs">
          <Icon name="check" size={11} />
          <span>{notice}</span>
        </div>
      ) : mainReason ? (
        <div className="git-notice gated iflex-center text-xs">
          <Icon name="alert" size={11} />
          <span>{mainReason}</span>
        </div>
      ) : showIdle ? (
        <div className={`git-act-status flex-center text-xs ${statusKind}`}>
          <span className="d" />
          <span className="lbl">
            {idle != null ? (
              idle
            ) : panelState === "pushed" && pushedLink ? (
              <>
                <GitLink href={pushedLink}>
                  {aheadCount === 1 ? "1 commit" : `${aheadCount} commits`}
                </GitLink>
                {" pushed · no PR yet"}
              </>
            ) : (
              statusLabel
            )}
          </span>
        </div>
      ) : null}
      <SplitAction
        items={items}
        selectedKey={selectedKey}
        tone={tone}
        mainDisabled={mainDisabled}
        mainReason={mainReason}
        busyLabel={busy ?? (delegationLabel ? "Agent working…" : null)}
        onSelect={onSelect}
        onRun={onRun}
      />
    </div>
  );
}
