import { StartButton, type WalkthroughStart } from "./Walkthrough/StartButton";

interface ChangesHeaderProps {
  /** The list's totals; null when there is no list to total (nothing changed
   *  in this scope) and the row only carries the way into a walkthrough. */
  totals: { count: number; additions: number; deletions: number } | null;
  /** Follow's state and its switch; omitted where there is no list to follow
   *  the agent through. */
  follow?: { on: boolean; busy: boolean; onToggle: () => void };
  /** The way into the walkthrough, when there is one to start or resume. */
  walkthrough: WalkthroughStart | null;
  /** The fork-point list could not be fetched; the list is uncommitted only. */
  degraded: boolean;
}

/** One row over the list: its totals, then what the view can do right now —
 *  follow the agent, walk through the change — and, when the scope's own list
 *  is unavailable, one line saying what is shown instead.
 *
 *  Follow shows only when it has something to say: during a turn (it is live,
 *  or paused), and after one while it is still paused, so it can be re-armed
 *  for the next. Armed with nothing to follow is the resting state and draws
 *  nothing. */
export function ChangesHeader({ totals, follow, walkthrough, degraded }: ChangesHeaderProps) {
  return (
    <>
      <div className="code-h flex-center">
        <div className="code-h-l text-sm">
          {totals && (
            <>
              <span className="ch-count">
                {totals.count}
                <span className="dim"> {totals.count === 1 ? "file" : "files"}</span>
              </span>
              <span className="ch-dot">·</span>
              <span className="ch-add">+{totals.additions}</span>
              <span className="ch-rem">−{totals.deletions}</span>
            </>
          )}
        </div>
        {follow && (follow.busy || !follow.on) && (
          <FollowToggle follow={follow.on} busy={follow.busy} onToggle={follow.onToggle} />
        )}
        {walkthrough && <StartButton {...walkthrough} />}
      </div>
      {degraded && <DegradedHint />}
    </>
  );
}

/** Armed, Follow opens whatever file the agent is editing during a turn. */
function FollowToggle({
  follow,
  busy,
  onToggle,
}: {
  follow: boolean;
  busy: boolean;
  onToggle: () => void;
}) {
  return (
    <button
      className={`code-follow ${follow ? "on" : "off"} ${busy ? "live" : ""} tip text-xs`}
      data-tip={
        follow ? "Auto-following the agent's current file" : "Click to auto-follow agent edits"
      }
      aria-pressed={follow}
      onClick={onToggle}
    >
      <span className="cf-dot"></span>
      <span>{follow ? (busy ? "Following live" : "Following") : "Paused"}</span>
    </button>
  );
}

export function DegradedHint() {
  return (
    <div className="code-degraded text-xs">
      Couldn't list changes since the fork — showing uncommitted files only.
    </div>
  );
}

/** Git is paused in a checkout whose config tells git to run a program
 *  (`git::hardening`): the host can't read it, so no list here speaks for it.
 *  `checkouts` names them for a multi-repo agent; null for the only checkout.
 *  The Git panel is where the keys are shown and removed. */
export function BlockedHint({ checkouts }: { checkouts: string[] | null }) {
  const where = checkouts ? `in ${checkouts.join(", ")}` : "in this checkout";
  return (
    <div className="code-degraded text-xs">
      Git is paused {where}: its config tells git to run a program, so its changes can't be read.
      The Git panel shows the settings and removes them.
    </div>
  );
}
