import { Icon } from "../../Icon";
import type { ActionTone, GitPanelState, StatusKind } from "../primaryActions";
import { SplitAction, type SplitActionItem } from "./SplitAction";
import { GitLink, Spinner, ViewOnGitHub } from "./shared";

/** The pinned footer's action row: a single status slot (busy spinner →
 *  delegation → transient notice → the idle primary status) followed by the
 *  split action button. */
export function ActionBar({
  statusKind,
  statusLabel,
  statusExtra,
  busy,
  delegationLabel,
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
          <span className="lbl">{delegationLabel}</span>
        </div>
      ) : notice ? (
        <div className="git-notice iflex-center text-xs">
          <Icon name="check" size={11} />
          <span>{notice}</span>
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
