import { open } from "@tauri-apps/plugin-shell";
import type { PrChecks, PrState } from "@/api";
import { Icon } from "@/components/Icon";
import { Badge, type BadgeVariant } from "@/components/ui/Badge";

/** One PR chip in the strip. `context` names what this PR belongs to — the repo
 *  of a multi-repo set ("Frontend") — and is what the tooltip and screen-reader
 *  label lead with. */
export interface PrSetEntry {
  key: string;
  context: string;
  pr: PrState;
  checks: PrChecks | null;
}

/** The status pill for one PR of the set: state first, refined by the CI
 *  rollup while open (same tint semantics as the sidebar's PR pill). Shared
 *  with the per-checkout `PrSwitcher`, so a PR reads the same in both. */
export function chipStatus(
  pr: PrState,
  checks: PrChecks | null,
): { variant: BadgeVariant; word: string } {
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

/** Slim strip of linked PR pills above a multi-repo panel, under a short
 *  heading: one task's PRs across two or more repos ("3 PRs"), each one click
 *  from GitHub. A single checkout's own PRs are the `PrSwitcher`'s. */
export function PrSetStrip({ heading, entries }: { heading: string; entries: PrSetEntry[] }) {
  return (
    <div className="git-pr-set text-xs">
      <span className="git-pr-set-label">{heading}</span>
      {entries.map(({ key, context, pr, checks }) => {
        const { variant, word } = chipStatus(pr, checks);
        return (
          <button
            key={key}
            className="git-pr-set-chip"
            onClick={() => pr.url && void open(pr.url)}
            aria-label={`${context} PR #${pr.number} — ${word}`}
          >
            <Badge variant={variant} tip={`${context} · ${word}`}>
              <Icon name={pr.state === "merged" ? "merge" : "pr"} size={10} />#{pr.number}
            </Badge>
          </button>
        );
      })}
    </div>
  );
}
