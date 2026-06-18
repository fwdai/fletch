import { formatTokens, formatCost } from "../../util/format";

export interface AgentStats {
  launched: string;
  runtime: string;
  /** Context-window fill in tokens, or null when the agent reports no usage. */
  contextTokens: number | null;
  /** Context window size in tokens (provider-reported or default). */
  contextWindow: number;
  contextPct: number;
  /** Cumulative session totals, or null when the agent reports no usage. */
  totalInput: number | null;
  totalOutput: number | null;
  /** Cumulative dollar cost, or null when no usage / the agent doesn't report cost. */
  costUsd: number | null;
}

/** Floats off the agent-row time chip on hover. Visual lift only —
 *  pure stats display, no actions. */
export function AgentStatsPopover({ stats }: { stats: AgentStats }) {
  const hasUsage = stats.contextTokens != null;
  const contextLabel = hasUsage
    ? `${formatTokens(stats.contextTokens!)} / ${formatTokens(stats.contextWindow)}`
    : "—";
  const ioLabel =
    stats.totalInput != null && stats.totalOutput != null
      ? `${formatTokens(stats.totalInput)} in · ${formatTokens(stats.totalOutput)} out`
      : "—";

  return (
    <div className="ag-stats-pop" onClick={(e) => e.stopPropagation()}>
      <Row label="Launched" value={stats.launched} />
      <Row label="Runtime" value={stats.runtime} />
      <Row label="Context" value={contextLabel} />
      <div className="st-bar">
        <div className="st-bar-fill" style={{ width: `${stats.contextPct}%` }} />
      </div>
      <Row label="Context used" value={`${stats.contextPct}%`} />
      <Row label="Tokens" value={ioLabel} />
      {stats.costUsd != null && stats.costUsd > 0 && (
        <Row label="Cost" value={formatCost(stats.costUsd)} />
      )}
    </div>
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
