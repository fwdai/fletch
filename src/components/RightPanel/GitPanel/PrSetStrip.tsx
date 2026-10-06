import { open } from "@tauri-apps/plugin-shell";
import type { PrChecks, PrState } from "@/api";
import { Icon } from "@/components/Icon";
import { Badge } from "@/components/ui/Badge";
import { prTint } from "@/util/prState";

/** One PR chip in the strip. `context` names what this PR belongs to — the repo
 *  of a multi-repo set ("Frontend") — and is what the tooltip and screen-reader
 *  label lead with. */
export interface PrSetEntry {
  key: string;
  context: string;
  pr: PrState;
  checks: PrChecks | null;
}

/** Slim strip of linked PR pills above a multi-repo panel, under a short
 *  heading: one task's PRs across two or more repos ("3 PRs"), each one click
 *  from GitHub. A single checkout's own PRs are the `PrSwitcher`'s. */
export function PrSetStrip({ heading, entries }: { heading: string; entries: PrSetEntry[] }) {
  return (
    <div className="git-pr-set text-xs">
      <span className="git-pr-set-label">{heading}</span>
      {entries.map(({ key, context, pr, checks }) => {
        const { variant, word } = prTint(pr, checks);
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
