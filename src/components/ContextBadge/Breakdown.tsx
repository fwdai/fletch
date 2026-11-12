import type { ReactNode } from "react";
import { hasUsage, totalTokens, type UsageSnapshot } from "@/adapters/usage";
import { formatCost, formatTokensK } from "@/util/format";
import type { ContextView } from "./model";

/** The card under the chip. Its two halves answer different questions and are
 *  labelled apart. The bar is the LIVE context window: one measurement of the
 *  last turn. The rows are the SESSION TOTAL: every call the session made,
 *  summed — a long session's total dwarfs its window, and that's correct, the
 *  cached prefix is paid for again on every turn. */
export function Breakdown({
  usage,
  view,
  cost,
  footer,
}: {
  usage: UsageSnapshot;
  view: ContextView;
  /** The session's dollar cost: stated by the agent, or estimated from the
   *  catalog's list prices. Null when neither can price it. */
  cost: number | null;
  /** The compact action or receipt; absent for an agent that can't compact. */
  footer?: ReactNode;
}) {
  const { tokens } = usage.spend;
  const reset = usage.context.state === "reset";
  const spent = hasUsage(usage);
  return (
    <div className="usage-pop">
      <div className="up-head">
        <span className="up-title text-xs">Context window</span>
        <span className="up-frac text-xs">
          {view.pct == null ? (
            reset ? (
              "compacted"
            ) : spent ? (
              "unknown"
            ) : (
              "not measured yet"
            )
          ) : (
            <>
              <b>{formatTokensK(view.used)}</b> / {formatTokensK(view.max)}
            </>
          )}
        </span>
      </div>

      {view.pct == null ? (
        <div className="up-note text-sm">
          {reset
            ? "The next turn measures the new window."
            : spent
              ? "This agent doesn't report context usage."
              : "The agent's first response measures the window."}
        </div>
      ) : (
        <WindowBar view={view} />
      )}

      <div className="up-sep" />

      <div className="up-head">
        <span className="up-title text-xs">Session total</span>
      </div>
      <div className="up-rows">
        <Row label="Input" value={formatTokensK(tokens.input)} />
        <Row label="Output" value={formatTokensK(tokens.output)} />
        <Row label="Cache read" value={formatTokensK(tokens.cacheRead)} />
        <Row label="Cache write" value={formatTokensK(tokens.cacheWrite)} />
        <Row label="All tokens" value={formatTokensK(totalTokens(tokens))} total />
        {cost != null && (
          <Row
            label={usage.spend.costUsd != null ? "Cost" : "Est. cost"}
            value={formatCost(cost)}
            total
          />
        )}
      </div>

      {usage.coverage === "partial" && (
        <div className="up-note text-sm">
          This agent reports usage only while running, so turns from before Fletch was opened aren't
          counted.
        </div>
      )}

      {footer && (
        <>
          <div className="up-sep" />
          {footer}
        </>
      )}
    </div>
  );
}

function WindowBar({ view }: { view: ContextView }) {
  return (
    <>
      <div className="up-bar">
        {view.segments.map((s) => (
          <span key={s.key} className="up-seg" style={{ flex: s.tokens, background: s.color }} />
        ))}
        <span className="up-seg track" style={{ flex: view.free }} />
      </div>
      <div className="up-legend">
        {view.segments.map((s) => (
          <div key={s.key} className="up-leg flex-center text-sm">
            <span className="up-dot" style={{ background: s.color }} />
            <span className="up-k">{s.label}</span>
            <span className="up-v">{formatTokensK(s.tokens)}</span>
          </div>
        ))}
        <div className="up-leg flex-center text-sm">
          <span className="up-dot track" />
          <span className="up-k">Free</span>
          <span className="up-v">{formatTokensK(view.free)}</span>
        </div>
      </div>
    </>
  );
}

function Row({ label, value, total }: { label: string; value: string; total?: boolean }) {
  return (
    <div className={`up-row flex-center text-sm${total ? " total" : ""}`}>
      <span>{label}</span>
      <span className="up-rv">{value}</span>
    </div>
  );
}
