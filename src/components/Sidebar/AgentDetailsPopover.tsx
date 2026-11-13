import type { RefObject } from "react";
import type { ContextState } from "@/adapters/usage";
import type { AgentRecord } from "@/api";
import { AgentIdentityChip, useAgentIdentityLabel } from "@/components/AgentIdentityChip";
import { IsolationDetail } from "@/components/TitleBar/WorkspaceStatus/IsolationDetail";
import { AnchoredPopover } from "@/components/ui";
import { formatCost, formatTokens } from "@/util/format";

export interface AgentStats {
  /** Context-window fill in tokens, or null when it isn't known. */
  contextTokens: number | null;
  /** Context window size in tokens (provider-reported or default). */
  contextWindow: number;
  /** Percentage of the window in use, or null when the fill isn't known —
   *  never coerce that to 0, which reads as "empty" rather than "unknown". */
  contextPct: number | null;
  contextState: ContextState;
  /** Cumulative session totals, or null when the agent reports no usage. */
  totalInput: number | null;
  totalOutput: number | null;
  /** Cumulative dollar cost, or null when no usage / the agent doesn't report cost. */
  costUsd: number | null;
}

/** The agent row's one details card, floated off its sandbox glyph
 *  (`anchorRef`) on hover, and kept to a glance: the sandbox and its state,
 *  the agent in it, then what that has used. A usage line is there only once
 *  there is a number to put in it. Display only, but the pointer can move
 *  into it to select text. */
export function AgentDetailsPopover({
  agent,
  stats,
  anchorRef,
}: {
  agent: AgentRecord;
  stats: AgentStats;
  anchorRef: RefObject<HTMLElement | null>;
}) {
  // The custom agent's name, the model and the effort; the provider's name
  // until a model is known.
  const identity = useAgentIdentityLabel(agent);
  const context =
    stats.contextTokens != null && stats.contextPct != null
      ? `${formatTokens(stats.contextTokens)} / ${formatTokens(stats.contextWindow)} · ${stats.contextPct}%`
      : stats.contextState === "reset"
        ? "compacted"
        : null;
  const tokens =
    stats.totalInput != null && stats.totalOutput != null
      ? `${formatTokens(stats.totalInput)} in · ${formatTokens(stats.totalOutput)} out`
      : null;
  const cost = stats.costUsd != null && stats.costUsd > 0 ? formatCost(stats.costUsd) : null;

  return (
    <AnchoredPopover
      anchorRef={anchorRef}
      className="ag-details-pop"
      // Body-level in the DOM, but still the row's child to React: a click
      // here would otherwise select the row.
      onClick={(e) => e.stopPropagation()}
    >
      <IsolationDetail agent={agent} compact />
      <div className="ws-pop-iso">
        <AgentIdentityChip agent={agent} size={12} tip={false} />
        <div className="ws-pop-iso-text">
          <div className="ws-pop-iso-title">{identity}</div>
        </div>
      </div>
      {(context || tokens || cost) && (
        <div className="st-rows">
          {context && <Row label="Context" value={context} />}
          {tokens && <Row label="Tokens" value={tokens} />}
          {cost && <Row label="Cost" value={cost} />}
        </div>
      )}
    </AnchoredPopover>
  );
}

function Row({ label, value }: { label: string; value: string }) {
  return (
    <div className="st-row">
      <span className="st-k">{label}</span>
      <span className="st-v">{value}</span>
    </div>
  );
}
