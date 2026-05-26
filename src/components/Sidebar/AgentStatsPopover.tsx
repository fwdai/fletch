import { formatTokens } from "../../util/format";

export interface AgentStats {
  launched: string;
  runtime: string;
  tokens: number | null;
  contextPct: number;
}

/** Floats off the agent-row time chip on hover. Visual lift only —
 *  pure stats display, no actions. */
export function AgentStatsPopover({ stats }: { stats: AgentStats }) {
  const tokenLabel = stats.tokens != null ? `${formatTokens(stats.tokens)} / 200k` : "—";
  return (
    <div className="ag-stats-pop" onClick={(e) => e.stopPropagation()}>
      <Row label="Launched" value={stats.launched} />
      <Row label="Runtime" value={stats.runtime} />
      <Row label="Last turn" value={tokenLabel} />
      <div className="st-bar">
        <div className="st-bar-fill" style={{ width: `${stats.contextPct}%` }} />
      </div>
      <Row label="Context used" value={`${stats.contextPct}%`} />
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
