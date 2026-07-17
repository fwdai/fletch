import { open } from "@tauri-apps/plugin-shell";
import type { PrChecks, PrState, TrackedRepo } from "@/api";
import { Icon } from "@/components/Icon";
import { Badge, type BadgeVariant } from "@/components/ui/Badge";
import { basename } from "@/util/format";

/** One repo's PR for the strip: the panel section's own resolved state. */
export interface PrSetEntry {
  repo: TrackedRepo;
  pr: PrState;
  checks: PrChecks | null;
}

/** The status pill for one PR of the set: state first, refined by the CI
 *  rollup while open (same tint semantics as the sidebar's PR pill). */
function chipStatus(pr: PrState, checks: PrChecks | null): { variant: BadgeVariant; word: string } {
  if (pr.state === "merged") return { variant: "pr-merged", word: "merged" };
  if (pr.state === "closed") return { variant: "pr-closed", word: "closed" };
  switch (checks?.rollup) {
    case "passing":
      return { variant: "pr-pass", word: "checks passing" };
    case "failing":
      return { variant: "pr-fail", word: "checks failing" };
    case "pending":
      return { variant: "pr-open", word: "checks running" };
    default:
      return { variant: "pr-open", word: "open" };
  }
}

/** Slim summary strip above a multi-repo agent's panel sections when the task
 *  produced PRs in two or more repos: "2 PRs" plus one linked pill per PR, so
 *  the set reads as a unit and each PR is one click away. Rendered only by
 *  `MultiRepoGitPanel` (never for single-repo agents). */
export function PrSetStrip({ entries }: { entries: PrSetEntry[] }) {
  return (
    <div className="git-pr-set text-xs">
      <span className="git-pr-set-label">{entries.length} PRs</span>
      {entries.map(({ repo, pr, checks }) => {
        const { variant, word } = chipStatus(pr, checks);
        const label = repo.label ?? basename(repo.repo_path);
        return (
          <button
            key={repo.subdir}
            className="git-pr-set-chip"
            onClick={() => pr.url && void open(pr.url)}
            aria-label={`${label} PR #${pr.number} — ${word}`}
          >
            <Badge variant={variant} tip={`${label} · ${word}`}>
              <Icon name={pr.state === "merged" ? "merge" : "pr"} size={10} />#{pr.number}
            </Badge>
          </button>
        );
      })}
    </div>
  );
}
