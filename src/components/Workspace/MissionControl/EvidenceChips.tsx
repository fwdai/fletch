import { open } from "@tauri-apps/plugin-shell";
import type { PrChecks } from "@/api";
import { Icon } from "@/components/Icon";
import type { ReviewItem } from "./queue";

/** The evidence row under a card's identity line: one chip per signal we
 *  actually have. Everything degrades gracefully — a missing signal (no diff, no
 *  checks, unknown budget) renders nothing rather than a fake zero or a broken
 *  chip. */
export function EvidenceChips({ item }: { item: ReviewItem }) {
  const chips = [
    item.diff && (item.diff.additions > 0 || item.diff.deletions > 0) ? (
      <span key="diff" className="mc-chip" title={`${item.diff.file_count} file(s) changed`}>
        <Icon name="diff" size={11} />
        {item.diff.additions > 0 && <span className="add">+{item.diff.additions}</span>}
        {item.diff.deletions > 0 && <span className="rem">−{item.diff.deletions}</span>}
        <span className="mc-chip-sub">
          {item.diff.file_count} {item.diff.file_count === 1 ? "file" : "files"}
        </span>
      </span>
    ) : null,
    item.checks ? <ChecksChip key="checks" checks={item.checks} /> : null,
    typeof item.unresolvedComments === "number" && item.unresolvedComments > 0 ? (
      <span key="comments" className="mc-chip">
        <Icon name="bot" size={11} />
        {item.unresolvedComments} unresolved
      </span>
    ) : null,
    item.pr ? (
      <button
        key="pr"
        type="button"
        className="mc-chip mc-chip-link"
        title={`Open PR #${item.pr.number} on GitHub`}
        onClick={(e) => {
          e.stopPropagation();
          if (item.pr) void open(item.pr.url);
        }}
      >
        <Icon name="pr" size={11} />#{item.pr.number}
      </button>
    ) : null,
    // Future-PR slot (§1): a staleness chip, wired here so it lands with no
    // layout change when a later PR feeds `item.staleness`. Null today.
    item.staleness ? (
      <span key="stale" className="mc-chip mc-chip-warn">
        <Icon name="branch" size={11} />
        behind {item.staleness.base} by {item.staleness.behind}
      </span>
    ) : null,
  ].filter(Boolean);

  if (chips.length === 0) return null;
  return <div className="mc-chips">{chips}</div>;
}

/** CI rollup chip — tinted only on a settled pass/fail; pending stays neutral;
 *  a rollup of "none" (no checks configured) shows nothing. */
function ChecksChip({ checks }: { checks: PrChecks }) {
  if (checks.rollup === "none") return null;
  const tone =
    checks.rollup === "passing"
      ? "mc-chip-ok"
      : checks.rollup === "failing"
        ? "mc-chip-fail"
        : "mc-chip-pending";
  const label =
    checks.rollup === "passing"
      ? `checks passing (${checks.passed}/${checks.total})`
      : checks.rollup === "failing"
        ? `${checks.failed} ${checks.failed === 1 ? "check" : "checks"} failing`
        : `${checks.pending} pending`;
  return (
    <span className={`mc-chip ${tone}`}>
      <Icon name="flask" size={11} />
      {label}
    </span>
  );
}
