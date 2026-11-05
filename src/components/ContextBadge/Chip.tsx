import { Icon } from "@/components/Icon";
import { formatTokensK } from "@/util/format";
import type { CompactPhase, ContextView } from "./model";
import { Ring } from "./Ring";

/** What the ring shows while compacting an unmeasured window: a short arc, so
 *  the spin reads as motion rather than a bare track. */
const SPIN_ARC_PCT = 25;

/** The badge's main hit target: ring, used / max, caret. Clicking it pins the
 *  breakdown open. Nothing else lives here — cost and the split are in the
 *  breakdown. */
export function Chip({
  view,
  phase,
  open,
  onClick,
}: {
  view: ContextView;
  phase: CompactPhase;
  open: boolean;
  onClick: () => void;
}) {
  const max = formatTokensK(view.max);
  const label =
    view.pct == null
      ? `Context usage unknown, ${max} window`
      : `Context ${formatTokensK(view.used)} of ${max} used (${view.pct}%)`;
  return (
    <button
      type="button"
      className="husage-chip"
      aria-label={label}
      aria-expanded={open}
      onClick={onClick}
    >
      <Ring pct={phase === "compacting" ? (view.pct ?? SPIN_ARC_PCT) : view.pct} />
      <span className="hu-frac">
        {phase === "compacting" ? (
          <span className="hu-busy">Compacting…</span>
        ) : (
          <>
            <b>{view.pct == null ? "—" : formatTokensK(view.used)}</b>
            <i>/</i>
            {max}
          </>
        )}
      </span>
      <Icon name="chevD" size={9} className="hu-caret" />
    </button>
  );
}
