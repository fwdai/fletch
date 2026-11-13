import type { RefObject } from "react";
import type { ContextState } from "@/adapters/usage";
import type { AgentRecord } from "@/api";
import { AgentIdentityChip, useAgentIdentityLabel } from "@/components/AgentIdentityChip";
import { IsolationDetail } from "@/components/TitleBar/WorkspaceStatus/IsolationDetail";
import { AnchoredPopover } from "@/components/ui";
import { providerLabel } from "@/data/providers";
import { formatCost, formatTokens } from "@/util/format";

export interface AgentStats {
  launched: string;
  runtime: string;
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
 *  (`anchorRef`) on hover. Dense, in two parts, each fact under what it is
 *  about: the sandbox, its state and since when the workspace has run, then
 *  the agent in it and what that has used. Display only, but the pointer can
 *  move into it to select text. */
export function AgentDetailsPopover({
  agent,
  stats,
  anchorRef,
}: {
  agent: AgentRecord;
  stats: AgentStats;
  anchorRef: RefObject<HTMLElement | null>;
}) {
  const provider = providerLabel(agent.provider);
  // The custom agent's name, the model and the effort; just the provider's
  // name again until a model is known.
  const identity = useAgentIdentityLabel(agent);
  // The fill and its share of the window on one line; the bar draws the share.
  const contextLabel =
    stats.contextTokens != null
      ? [
          `${formatTokens(stats.contextTokens)} / ${formatTokens(stats.contextWindow)}`,
          stats.contextPct != null && `${stats.contextPct}%`,
        ]
          .filter(Boolean)
          .join(" · ")
      : stats.contextState === "reset"
        ? "compacted"
        : "—";
  const ioLabel =
    stats.totalInput != null && stats.totalOutput != null
      ? `${formatTokens(stats.totalInput)} in · ${formatTokens(stats.totalOutput)} out`
      : "—";

  return (
    <AnchoredPopover
      anchorRef={anchorRef}
      className="ag-details-pop"
      // Body-level in the DOM, but still the row's child to React: a click
      // here would otherwise select the row.
      onClick={(e) => e.stopPropagation()}
    >
      <IsolationDetail agent={agent} compact>
        <div className="st-rows">
          <Row label="Launched" value={stats.launched} />
          <Row label="Runtime" value={stats.runtime} />
        </div>
      </IsolationDetail>
      <div className="ws-pop-div" />
      <div className="ws-pop-iso">
        <AgentIdentityChip agent={agent} size={12} tip={false} />
        <div className="ws-pop-iso-text">
          <div className="ws-pop-iso-title">{provider}</div>
          {identity !== provider && <div className="ws-pop-iso-hint">{identity}</div>}
          <div className="st-rows">
            <Row label="Context" value={contextLabel} />
            <div className="st-bar">
              <div className="st-bar-fill" style={{ width: `${stats.contextPct ?? 0}%` }} />
            </div>
            <Row label="Tokens" value={ioLabel} />
            {stats.costUsd != null && stats.costUsd > 0 && (
              <Row label="Cost" value={formatCost(stats.costUsd)} />
            )}
          </div>
        </div>
      </div>
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
