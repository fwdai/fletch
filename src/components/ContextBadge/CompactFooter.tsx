import { Icon } from "@/components/Icon";
import { formatTokensK } from "@/util/format";
import type { CompactPhase } from "./model";

/** The breakdown's last row: the compact action, which is always here (the
 *  shrink segment is the shortcut to it), or — once a compaction has landed —
 *  a receipt for what it gave back. */
export function CompactFooter({
  phase,
  freed,
  blocker,
  keys,
  onCompact,
}: {
  phase: CompactPhase;
  /** Tokens the last compaction freed, when the transcript stated them. */
  freed: number | null;
  blocker: string | null;
  keys: string;
  onCompact: () => void;
}) {
  if (phase === "done") {
    return (
      <div className="up-compact done">
        <span className="up-cdot" aria-hidden="true" />
        <span className="up-ctext">
          <span className="up-ct">Conversation compacted</span>
          <span className="up-cs">
            {freed
              ? `Freed ${formatTokensK(freed)} · earlier turns summarized`
              : "Earlier turns summarized"}
          </span>
        </span>
      </div>
    );
  }

  const compacting = phase === "compacting";
  return (
    <button
      type="button"
      className="up-compact"
      aria-disabled={blocker != null}
      onClick={() => {
        if (!blocker) onCompact();
      }}
    >
      <Icon name="compact" size={13} strokeWidth={1.75} />
      <span className="up-ctext">
        <span className="up-ct">{compacting ? "Compacting…" : "Compact conversation"}</span>
        <span className="up-cs">
          {!compacting && blocker ? blocker : "Summarize earlier turns"}
        </span>
      </span>
      {keys && <kbd className="up-kbd">{keys}</kbd>}
    </button>
  );
}
