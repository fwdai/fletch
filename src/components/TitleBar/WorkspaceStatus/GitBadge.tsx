import type { GitState, PrChecks, PrSetEntry, PrState, ShortStats } from "@/api";
import { Icon } from "@/components/Icon";
import { detectBlockers } from "@/readiness";
import { plural } from "@/util/format";
import { type PrTint, prTint, summarizePrSet } from "@/util/prSummary";

interface Props {
  pr: PrState | null;
  git: GitState | null;
  checks: PrChecks | null;
  stats: ShortStats | null;
  /** The checkout's whole PR set, focused included. */
  prs: readonly PrSetEntry[];
}

/** The badge's tint class for a PR tint — the app-wide vocabulary
 *  (`prTint`), with the checkout's own conflict markers folded in: a local
 *  conflict is the checkout's, and only the focused PR's badge should read it. */
function badgeClass(tint: PrTint["variant"], localConflict: boolean): string {
  if (localConflict || tint === "warn") return "conflicts";
  switch (tint) {
    case "pr-fail":
      return "failing";
    case "pr-merged":
      return "merged";
    case "pr-closed":
      return "closed";
    default:
      return "open";
  }
}

/** The one badge that summarizes the checkout: PR (state-tinted, with number)
 *  once one exists, else the uncommitted diff, else the commits that have not
 *  left the checkout or have no PR yet, else a clean-tree check. The commits
 *  are read off the Git panel's own classification (`detectBlockers`), so the
 *  badge and the panel's ladder name the same step. With several PRs it keeps
 *  the focused `#N`, adds a quiet `+K` for the rest, and takes the set's worst
 *  tint — the popover lists them all. */
export function GitBadge({ pr, git, checks, stats, prs }: Props) {
  if (pr) {
    const localConflict = git?.files.some((f) => f.kind === "conflicted") ?? false;
    const own = badgeClass(prTint(pr, checks).variant, localConflict);
    const others = prs.length - 1;
    // A set takes its worst tint when that is failing CI or a conflict, so a
    // sibling in trouble never hides behind a calm focused PR.
    const set = others > 0 ? summarizePrSet(prs).variant : null;
    const cls = set === "pr-fail" ? "failing" : set === "warn" ? "conflicts" : own;
    return (
      <span className={`ws-badge pr-${cls}`}>
        <Icon name={pr.state === "merged" ? "merge" : "branch"} size={11} />
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
  // Committed work is not a clean checkout until it is pushed and proposed.
  const blockers = detectBlockers({ git, pr: null, checks: null, comments: null });
  const unpushed = blockers.find((b) => b.kind === "unpushed");
  if (unpushed?.kind === "unpushed") {
    return (
      <span className="ws-badge commits" title={`${plural(unpushed.commits, "commit")} not pushed`}>
        <Icon name="arrowUp" size={11} />
        <span className="mono">{unpushed.commits}</span> unpushed
      </span>
    );
  }
  if (git && blockers.some((b) => b.kind === "unsubmitted")) {
    return (
      <span
        className="ws-badge commits"
        title={`${plural(git.ahead, "commit")} ahead of ${git.parent_branch}, no pull request yet`}
      >
        <Icon name="commit" size={11} />
        <span className="mono">{git.ahead}</span> ahead
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
