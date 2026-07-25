import { useState } from "react";
import { contextPercent, resolveContextWindow, totalTokens } from "@/adapters/usage";
import { type UsageSnapshot, useAppStore } from "@/store";
import { formatCost, formatTokens } from "@/util/format";

/** Laconic context gauge for the composer foot — a donut ring + %, hover for a
 *  full breakdown. Mirrors the v2 design (.usage / .up-* styles in app.css).
 *
 *  The two halves of the popover answer different questions and are labelled
 *  apart. The ring and the segmented bar are the LIVE context window: one
 *  measurement of the last turn, split by cache state (reused / newly cached /
 *  fresh input — the truthful equivalent of the design's mocked
 *  system/conversation/reasoning split, which no agent's transcript exposes).
 *  The rows are the SESSION TOTAL: every API call the session made, summed. A
 *  long session's total dwarfs its window, and that's correct — the cached
 *  prefix is paid for again on every turn. */
export function UsageMeter({ usage }: { usage: UsageSnapshot }) {
  const [open, setOpen] = useState(false);
  const catalog = useAppStore((s) => s.modelCatalog);

  const contextWindow = resolveContextWindow(usage, catalog);
  const pct = contextPercent(usage, contextWindow);
  const used = usage.context.tokens;
  const free = Math.max(0, contextWindow - used);
  const { tokens, costUsd } = usage.spend;

  const segments = [
    {
      key: "cacheRead",
      label: "Cache read",
      tokens: usage.context.fill.cacheRead,
      color: "var(--accent)",
    },
    {
      key: "cacheWrite",
      label: "Cache write",
      tokens: usage.context.fill.cacheWrite,
      color: "var(--info)",
    },
    { key: "input", label: "Input", tokens: usage.context.fill.input, color: "var(--fg-2)" },
  ].filter((s) => s.tokens > 0);

  // ring geometry — a 13px donut whose arc length encodes pct
  const R = 6.5;
  const C = 2 * Math.PI * R;
  const ringColor =
    pct == null
      ? "var(--fg-3)"
      : pct >= 90
        ? "var(--danger)"
        : pct >= 75
          ? "var(--warn)"
          : "var(--accent)";

  return (
    <div
      className="usage iflex-center"
      onMouseEnter={() => setOpen(true)}
      onMouseLeave={() => setOpen(false)}
    >
      <button
        type="button"
        className="usage-chip iflex-center"
        aria-label={pct == null ? "Context unknown" : `Context ${pct}% used`}
      >
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
            strokeDashoffset={C * (1 - (pct ?? 0) / 100)}
            transform="rotate(-90 9 9)"
          />
        </svg>
        <span className="usage-val text-xs">{pct == null ? "—" : `${pct}%`}</span>
      </button>

      {open && (
        <div className="usage-pop">
          <div className="up-head">
            <span className="up-title text-xs">Context window</span>
            <span className="up-frac text-xs">
              {pct == null ? (
                usage.context.state === "reset" ? (
                  "compacted"
                ) : (
                  "unknown"
                )
              ) : (
                <>
                  <b>{formatTokens(used)}</b> / {formatTokens(contextWindow)}
                </>
              )}
            </span>
          </div>

          {pct == null ? (
            <div className="up-note text-sm">
              {usage.context.state === "reset"
                ? "Conversation compacted. The next turn measures the new window."
                : "This agent doesn't report context usage."}
            </div>
          ) : (
            <>
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
            </>
          )}

          <div className="up-sep" />

          <div className="up-head">
            <span className="up-title text-xs">Session total</span>
          </div>

          <div className="up-rows">
            <Row label="Input" value={formatTokens(tokens.input)} />
            <Row label="Output" value={formatTokens(tokens.output)} />
            <Row label="Cache read" value={formatTokens(tokens.cacheRead)} />
            <Row label="Cache write" value={formatTokens(tokens.cacheWrite)} />
            <div className="up-row flex-center total text-sm">
              <span>All tokens</span>
              <span className="up-rv">{formatTokens(totalTokens(tokens))}</span>
            </div>
            {costUsd != null && (
              <div className="up-row flex-center total text-sm">
                <span>Cost</span>
                <span className="up-rv">{formatCost(costUsd)}</span>
              </div>
            )}
          </div>

          {usage.coverage === "partial" && (
            <div className="up-note text-sm">
              This agent reports usage only while running, so turns from before Fletch was opened
              aren't counted.
            </div>
          )}
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
