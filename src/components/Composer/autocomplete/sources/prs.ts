import { useEffect, useMemo, useRef, useState } from "react";
import type { PrSummary } from "../../../../api";
import type { AcPick, AcSource } from "../types";

/** How long a fetched PR list stays fresh before the next open refetches —
 *  long enough to absorb rapid open/close cycling, short enough to pick up
 *  newly-opened PRs within a session. */
const PR_CACHE_MS = 15_000;

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
  const [prs, setPrs] = useState<PrSummary[]>([]);

  const active = listPrs && query !== null && /^\d*$/.test(query) ? query : null;

  const matched = useMemo(() => (active === null ? [] : filterPrs(prs, active)), [active, prs]);

  // Fetch when the menu opens, but cache for a short window so rapid
  // open/close cycles (type `#`, delete, retype) don't queue a burst of
  // `gh pr list` processes. `lastFetch` is stamped before the call so even
  // in-flight reopens skip. Held in refs so an inline `listPrs` prop and the
  // timestamp don't refire the effect.
  const open = active !== null;
  const ref = useRef(listPrs);
  ref.current = listPrs;
  const lastFetch = useRef(0);
  useEffect(() => {
    if (!open || !ref.current) return;
    if (Date.now() - lastFetch.current < PR_CACHE_MS) return;
    lastFetch.current = Date.now();
    let alive = true;
    ref
      .current()
      .then((p) => {
        if (alive) setPrs(p);
      })
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, [open]);

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
