interface ChangesHeaderProps {
  count: number;
  additions: number;
  deletions: number;
  follow: boolean;
  busy: boolean;
  onToggleFollow: () => void;
  /** The fork-point list could not be fetched; the list is uncommitted only. */
  degraded: boolean;
}

/** Totals over the list, the Follow toggle, and — when the scope's own list
 *  is unavailable — one line saying what is shown instead. */
export function ChangesHeader({
  count,
  additions,
  deletions,
  follow,
  busy,
  onToggleFollow,
  degraded,
}: ChangesHeaderProps) {
  return (
    <>
      <div className="code-h flex-center">
        <div className="code-h-l text-sm">
          <span className="ch-count">
            {count}
            <span className="dim"> {count === 1 ? "file" : "files"}</span>
          </span>
          <span className="ch-dot">·</span>
          <span className="ch-add">+{additions}</span>
          <span className="ch-rem">−{deletions}</span>
        </div>
        <FollowToggle follow={follow} busy={busy} onToggle={onToggleFollow} />
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
