import { Icon } from "@/components/Icon";
import type { ActionTone, GitPanelState, StatusKind } from "@/components/RightPanel/primaryActions";
import { SplitAction, type SplitActionItem } from "./SplitAction";
import { GitLink, Spinner, ViewOnGitHub } from "./shared";

/** The pinned footer's action row: a single status slot (busy spinner →
 *  delegation → transient notice → why autopilot stopped → the idle primary
 *  status) followed by the split action button. */
export function ActionBar({
  statusKind,
  statusLabel,
  statusExtra,
  busy,
  delegationLabel,
  autoAttempt,
  autoStuck,
  notice,
  panelState,
  pushedLink,
  aheadCount,
  prUrl,
  items,
  selectedKey,
  tone,
  mainDisabled,
  onSelect,
  onRun,
}: {
  statusKind: StatusKind;
  statusLabel: string;
  statusExtra?: string;
  busy: string | null;
  delegationLabel: string | null;
  /** Set when the in-flight delegation was started by autopilot, not a click —
   *  the retry number of that cycle. A second or third try is exactly when the
   *  user wants to know before it gives up. */
  autoAttempt: number | null;
  /** Why autopilot stopped working on this PR by itself, or null while it
   *  hasn't. Phrased as a fact about the PR (see `stuckLabel`). */
  autoStuck: string | null;
  notice: string | null;
  panelState: GitPanelState;
  pushedLink: string | null;
  aheadCount: number;
  prUrl: string | undefined;
  items: SplitActionItem[];
  selectedKey: string;
  tone: ActionTone;
  mainDisabled: boolean;
  onSelect: (key: string) => void;
  onRun: () => void;
}) {
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
      ) : autoStuck ? (
        // Warn, not danger: nothing is broken, the agent did what it could and
        // the next step is a person's. The primary action below is that step.
        <div className="git-act-status flex-center warn text-xs">
          <span className="d" />
          <span className="lbl" title={autoStuck}>
            {autoStuck}
          </span>
        </div>
      ) : (
        <div className={`git-act-status flex-center text-xs ${statusKind}`}>
          <span className="d" />
          <span className="lbl">
            {panelState === "pushed" && pushedLink ? (
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
          {statusExtra && <span className="ex text-xs">{statusExtra}</span>}
          {/* View on GitHub is a convenience link, not an action — a quiet
              chip beside the status, never a menu item. */}
          {panelState === "pr-open" && prUrl && (
            <ViewOnGitHub href={prUrl} className="st-ext" size={11} />
          )}
        </div>
      )}
      <SplitAction
        items={items}
        selectedKey={selectedKey}
        tone={tone}
        mainDisabled={mainDisabled}
        busyLabel={busy ?? (delegationLabel ? "Agent working…" : null)}
        onSelect={onSelect}
        onRun={onRun}
      />
    </div>
  );
}
