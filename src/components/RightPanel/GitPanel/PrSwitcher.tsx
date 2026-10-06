import type { PrChecks, PrSetEntry, PrState } from "@/api";
import { Icon } from "@/components/Icon";
import { Badge, type BadgeVariant } from "@/components/ui/Badge";
import { useAppStore } from "@/store";

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

/** The checkout's PRs as a row of chips under the status header, one of them
 *  focused — the PR the header, card, checks, threads and action bar below all
 *  describe. Clicking another focuses it on the host (`focusPr`); the panel
 *  follows through the store's focused maps, with no wiring of its own.
 *
 *  Plain toggle buttons in the normal tab order, wrapping onto more lines when
 *  the set outgrows the panel. Rendered by `GitRepoSection` only once the
 *  checkout holds two or more PRs, so a one-PR panel is exactly what it was —
 *  and a host that reports a set has `set_focused_pr` (`focusPr` still refuses
 *  an older one). Each repo of a multi-repo panel has its own. */
export function PrSwitcher({
  agentId,
  subdir,
  entries,
  focused,
}: {
  agentId: string;
  subdir?: string;
  /** The checkout's PR set (`prSets`), newest number first. */
  entries: PrSetEntry[];
  /** The focused PR's number (`prStates`), or null while none is known. */
  focused: number | null;
}) {
  const focusPr = useAppStore((s) => s.focusPr);
  return (
    <div className="git-pr-switch" role="group" aria-label="Pull requests">
      {entries.map(({ state, checks }) => {
        const { variant, word } = chipStatus(state, checks);
        const selected = state.number === focused;
        // `#N · title · branch · status`, skipping what the host doesn't know.
        const summary = [`#${state.number}`, state.title, state.branch, word]
          .filter(Boolean)
          .join(" · ");
        return (
          <button
            key={state.number}
            type="button"
            aria-pressed={selected}
            className="git-pr-switch-chip"
            title={summary}
            aria-label={summary}
            onClick={selected ? undefined : () => void focusPr(agentId, state.number, subdir)}
          >
            <Badge variant={variant}>
              <Icon name={state.state === "merged" ? "merge" : "pr"} size={10} />#{state.number}
            </Badge>
          </button>
        );
      })}
    </div>
  );
}
