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
    // Staleness (§2): quiet information, not an alarm — a moved base is normal
    // in a parallel fleet, so it's muted, never danger-toned. Absent when the
    // base hasn't moved (or is unknown), so it renders nothing rather than 0.
    item.staleness ? (
      <span
        key="stale"
        className="mc-chip mc-chip-muted"
        title={`Base ${item.staleness.base} has moved ${item.staleness.behind} commit(s) ahead`}
      >
        <Icon name="branch" size={11} />
        base moved · {item.staleness.behind} behind
      </span>
    ) : null,
    // Overlap hints (§4): the quietest chip of all — advisory heads-up that
    // another agent on this repo touches some of the same files. Never a
    // warning color; no gating.
    ...(item.overlaps ?? []).map((o) => (
      <span
        key={`overlap:${o.agentName}`}
        className="mc-chip mc-chip-muted"
        title={`Overlaps with ${o.agentName} on ${o.count} file(s)`}
      >
        <Icon name="layers" size={11} />
        overlaps {o.agentName} ({o.count})
      </span>
    )),
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
