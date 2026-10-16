import type { ReactNode } from "react";
import { Icon } from "@/components/Icon";
import type { ActionTone } from "@/components/RightPanel/primaryActions";
import type { FooterAction, FooterGroup } from "./footerModel";
import { SplitAction } from "./SplitAction";
import { Spinner } from "./shared";

/** The pinned footer's action row: one status slot about *now* — a busy
 *  spinner, the agent's delegation, a transient notice, why the button is dead
 *  — then the split action button. What is wrong and what comes next is the
 *  ladder's to say, so at rest the slot carries only `idle`: the one note that
 *  is an affordance rather than a status (who writes the commit message). */
export function ActionBar({
  idle,
  busy,
  delegationLabel,
  autoAttempt,
  notice,
  primary,
  groups,
  tone,
  mainDisabled,
  mainReason,
  onPick,
  onRun,
}: {
  /** The resting note beside the button, when one is worth the space. */
  idle?: ReactNode;
  busy: string | null;
  delegationLabel: string | null;
  /** Set when the in-flight delegation was started by autopilot, not a click —
   *  the retry number of that cycle. A second or third try is exactly when the
   *  user wants to know before it gives up. */
  autoAttempt: number | null;
  notice: string | null;
  primary: FooterAction;
  groups: FooterGroup[];
  tone: ActionTone;
  mainDisabled: boolean;
  /** Why the primary can't run here — a capability the environment lacks, or
   *  a branch the checkout isn't on — which nothing the user does in the panel
   *  will change. Shown below the transient states, which still take
   *  precedence: what is happening now outranks what cannot happen at all. */
  mainReason?: string | null;
  onPick: (key: string) => void;
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
      ) : mainReason ? (
        <div className="git-notice gated iflex-center text-xs">
          <Icon name="alert" size={11} />
          <span>{mainReason}</span>
        </div>
      ) : idle != null ? (
        <div className="git-act-status flex-center text-xs">
          <span className="lbl">{idle}</span>
        </div>
      ) : null}
      <SplitAction
        primary={primary}
        groups={groups}
        tone={tone}
        mainDisabled={mainDisabled}
        mainReason={mainReason}
        busyLabel={busy ?? (delegationLabel ? "Agent working…" : null)}
        onPick={onPick}
        onRun={onRun}
      />
    </div>
  );
}
