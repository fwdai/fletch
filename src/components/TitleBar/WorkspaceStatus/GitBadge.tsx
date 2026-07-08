import type { GitState, PrChecks, PrState, ShortStats } from "@/api";
import { Icon } from "@/components/Icon";
import { PR_META, prBadge } from "./derive";

interface Props {
  pr: PrState | null;
  git: GitState | null;
  checks: PrChecks | null;
  stats: ShortStats | null;
}

/** The one badge that summarizes the checkout: PR (state-tinted, with number)
 *  once one exists, else the uncommitted diff, else a clean-tree check. */
export function GitBadge({ pr, git, checks, stats }: Props) {
  if (pr) {
    const meta = PR_META[prBadge(pr, git, checks)];
    return (
      <span className={`ws-badge pr-${meta.cls}`}>
        <Icon name={meta.icon} size={11} />
        <span className="mono">#{pr.number}</span>
      </span>
    );
  }
  const add = stats?.additions ?? 0;
  const rem = stats?.deletions ?? 0;
  if (add || rem) {
    return (
      <span className="ws-badge diff">
        <span className="add">+{add}</span>
        <span className="rem">−{rem}</span>
      </span>
    );
  }
  return (
    <span className="ws-badge clean" title="Working tree clean">
      <Icon name="check" size={11} /> clean
    </span>
  );
}

/** Collapses CI to its dominant state next to a PR badge: any failure wins,
 *  else any running, else all-passed. Full breakdown lives in the popover. */
export function ChecksChip({ checks }: { checks: PrChecks | null }) {
  if (!checks) return null;
  if (checks.failed > 0)
    return (
      <span className="ws-checks bad" title={`${checks.failed} failing`}>
        <Icon name="close" size={10} />
        {checks.failed}
      </span>
    );
  if (checks.pending > 0)
    return (
      <span className="ws-checks pend" title={`${checks.pending} running`}>
        <span className="ws-spin" />
        {checks.pending}
      </span>
    );
  if (checks.passed > 0)
    return (
      <span className="ws-checks ok" title="all checks passed">
        <Icon name="check" size={10} />
        {checks.passed}
      </span>
    );
  return null;
}
