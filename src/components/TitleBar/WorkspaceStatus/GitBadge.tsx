import type { GitState, PrChecks, PrSetEntry, PrState, ShortStats } from "@/api";
import { Icon } from "@/components/Icon";
import { summarizePrSet } from "@/util/prSummary";
import { PR_META, prBadge } from "./derive";

interface Props {
  pr: PrState | null;
  git: GitState | null;
  checks: PrChecks | null;
  stats: ShortStats | null;
  /** The checkout's whole PR set, focused included. */
  prs: readonly PrSetEntry[];
}

/** The one badge that summarizes the checkout: PR (state-tinted, with number)
 *  once one exists, else the uncommitted diff, else a clean-tree check. With
 *  several PRs it keeps the focused `#N`, adds a quiet `+K` for the rest, and
 *  takes the set's worst tint — the popover lists them all. */
export function GitBadge({ pr, git, checks, stats, prs }: Props) {
  if (pr) {
    const badge = prBadge(pr, git, checks);
    const meta = PR_META[badge];
    const others = prs.length - 1;
    // A set takes its worst tint when that is failing CI or a conflict, so a
    // sibling in trouble never hides behind a calm focused PR.
    const set = others > 0 ? summarizePrSet(prs).variant : null;
    const cls = set === "pr-fail" ? "failing" : set === "warn" ? "conflicts" : meta.cls;
    return (
      <span className={`ws-badge pr-${cls}`}>
        <Icon name={meta.icon} size={11} />
        <span className="mono">#{pr.number}</span>
        {others > 0 && (
          <span className="ws-badge-more mono" aria-label={`and ${others} more`}>
            +{others}
          </span>
        )}
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
