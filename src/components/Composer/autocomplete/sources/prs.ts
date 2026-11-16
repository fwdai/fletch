import { useMemo } from "react";
import type { PrSummary } from "@/api";
import type { AcPick, AcSource } from "@/components/Composer/autocomplete/types";
import { useCachedList } from "@/components/Composer/useCachedList";

/** Rank PRs against a numeric query: empty → most recent first; otherwise PRs
 *  whose number contains the digits, with prefix matches first. */
export function filterPrs(prs: PrSummary[], query: string, limit = 8): PrSummary[] {
  const recent = [...prs].sort((a, b) => b.number - a.number);
  if (!query) return recent.slice(0, limit);
  return recent
    .filter((p) => String(p.number).includes(query))
    .sort((a, b) => {
      const ap = String(a.number).startsWith(query) ? 0 : 1;
      const bp = String(b.number).startsWith(query) ? 0 : 1;
      return ap - bp || b.number - a.number;
    })
    .slice(0, limit);
}

interface Args {
  query: string | null;
  /** Lists the repo's open PRs. Omit to disable "#" mentions. */
  listPrs?: () => Promise<PrSummary[]>;
}

/** The "#" source: references a PR by number inline (e.g. `#123`), which the
 *  agent can resolve with `gh pr view`. Only fires on a digit query (empty
 *  allowed) so a "#" used in prose or a markdown heading doesn't pop the menu. */
export function usePrSource({ query, listPrs }: Args): AcSource {
  const active = listPrs && query !== null && /^\d*$/.test(query) ? query : null;
  // Fetched when the menu opens, cached briefly: retyping `#` must not queue
  // a burst of `gh pr list` processes.
  const prs = useCachedList(listPrs, active !== null);

  const matched = useMemo(
    () => (active === null || !prs ? [] : filterPrs(prs, active)),
    [active, prs],
  );

  const rows: AcSource["rows"] = matched.map((p) => ({
    title: `#${p.number}`,
    detail: p.title,
    icon: { glyph: "pr" },
  }));

  const pick = (i: number): AcPick | null => {
    const pr = matched[i];
    if (!pr) return null;
    return { replace: `#${pr.number} ` };
  };

  return { trigger: "#", heading: "Pull requests", query: active, rows, pick };
}
