import { useState } from "react";
import { lookupModel } from "../../data/modelCatalog";
import { type AgentUsage, useAppStore } from "../../store";
import { formatCost, formatTokens } from "../../util/format";

/** Laconic context gauge for the composer foot — a donut ring + %, hover for a
 *  full breakdown. Mirrors the v2 design (.usage / .up-* styles in app.css).
 *
 *  The segmented bar splits the CURRENT context window by cache state — reused
 *  (cache read), newly cached (cache write), fresh input — which is the
 *  truthful equivalent of the design's mocked system/conversation/reasoning
 *  split (that semantic split isn't recoverable from any agent's transcript).
 *  The rows below are SESSION cumulative totals. */
export function UsageMeter({ usage }: { usage: AgentUsage }) {
  const [open, setOpen] = useState(false);
  const catalog = useAppStore((s) => s.modelCatalog);

  // Prefer the window the agent reports for the live deployment (codex does);
  // otherwise look the model up in the catalog (claude/opencode/pi don't
  // report one); fall back to a default only when the model is unknown.
  const contextWindow =
    usage.contextWindow ||
    lookupModel(catalog, usage.model)?.contextWindow ||
    DEFAULT_CONTEXT_WINDOW;
  const used = usage.contextTokens;
  const free = Math.max(0, contextWindow - used);
  const pct = Math.min(100, Math.round((used / contextWindow) * 100));

  const segments = [
    {
      key: "cacheRead",
      label: "Cache read",
      tokens: usage.contextCacheRead,
      color: "var(--accent)",
    },
    {
      key: "cacheWrite",
      label: "Cache write",
      tokens: usage.contextCacheWrite,
      color: "var(--info)",
    },
    { key: "input", label: "Input", tokens: usage.contextInput, color: "var(--fg-2)" },
  ].filter((s) => s.tokens > 0);

  // ring geometry — a 13px donut whose arc length encodes pct
  const R = 6.5;
  const C = 2 * Math.PI * R;
  const ringColor = pct >= 90 ? "var(--danger)" : pct >= 75 ? "var(--warn)" : "var(--accent)";

  return (
    <div
      className="usage iflex-center"
      onMouseEnter={() => setOpen(true)}
      onMouseLeave={() => setOpen(false)}
    >
      <button type="button" className="usage-chip iflex-center" aria-label={`Context ${pct}% used`}>
        <svg className="usage-ring" viewBox="0 0 18 18" width="15" height="15">
          <circle cx="9" cy="9" r={R} fill="none" stroke="var(--bd-strong)" strokeWidth="2.2" />
          <circle
            cx="9"
            cy="9"
            r={R}
            fill="none"
            stroke={ringColor}
            strokeWidth="2.2"
            strokeLinecap="round"
            strokeDasharray={C}
            strokeDashoffset={C * (1 - pct / 100)}
            transform="rotate(-90 9 9)"
          />
        </svg>
        <span className="usage-val text-xs">{pct}%</span>
      </button>

      {open && (
        <div className="usage-pop">
          <div className="up-head">
            <span className="up-title text-xs">Context window</span>
            <span className="up-frac text-xs">
              <b>{formatTokens(used)}</b> / {formatTokens(contextWindow)}
            </span>
          </div>

          <div className="up-bar">
            {segments.map((s) => (
              <span
                key={s.key}
                className="up-seg"
                style={{ flex: s.tokens, background: s.color }}
              />
            ))}
            <span className="up-seg track" style={{ flex: free }} />
          </div>

          <div className="up-legend">
            {segments.map((s) => (
              <div key={s.key} className="up-leg flex-center text-sm">
                <span className="up-dot" style={{ background: s.color }} />
                <span className="up-k">{s.label}</span>
                <span className="up-v">{formatTokens(s.tokens)}</span>
              </div>
            ))}
            <div className="up-leg flex-center text-sm">
              <span className="up-dot track" />
              <span className="up-k">Free</span>
              <span className="up-v">{formatTokens(free)}</span>
            </div>
          </div>

          <div className="up-sep" />

          <div className="up-rows">
            <Row label="Input" value={formatTokens(usage.inputTokens)} />
            <Row label="Output" value={formatTokens(usage.outputTokens)} />
            {usage.costUsd > 0 && (
              <div className="up-row flex-center total text-sm">
                <span>Est. cost</span>
                <span className="up-rv">{formatCost(usage.costUsd)}</span>
              </div>
            )}
          </div>
        </div>
      )}
    </div>
  );
}

function Row({ label, value }: { label: string; value: string }) {
  return (
    <div className="up-row flex-center text-sm">
      <span>{label}</span>
      <span className="up-rv">{value}</span>
    </div>
  );
}

/** Fallback window for agents that don't report their own (claude/opencode/pi
 *  all run 200k-class models here); codex reports its own. */
const DEFAULT_CONTEXT_WINDOW = 200_000;
