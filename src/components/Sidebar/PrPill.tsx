import { Icon } from "@/components/Icon";
import { Badge } from "@/components/ui/Badge";
import type { AgentPr } from "@/util/prState";
import { summarizePrSet } from "@/util/prSummary";

/** The sidebar's PR pill, for an agent's whole set or one sub-agent's share of
 *  it: `#N` for one PR, `N PRs` for several, tinted by the worst open one (see
 *  `summarizePrSet`). The tooltip itemizes each PR so the pill stays a glance. */
export function PrPill({ prs }: { prs: readonly AgentPr[] }) {
  if (prs.length === 0) return null;
  const { variant, icon, label, tip } = summarizePrSet(prs);
  return (
    <Badge variant={variant} tip={tip}>
      <Icon name={icon} size={10} />
      {label}
    </Badge>
  );
}
