import { useState } from "react";
import { ProviderIcon } from "@/components/ProviderIcon";
import { formatHeatDay, Skeleton } from "@/components/Stats";
import { providerChip } from "@/data/providers";
import { type CostLabel, coverageLabel, modelCostLabel, type UsageStats } from "@/data/usage";
import { formatPercent, formatTokens } from "@/util/format";
import { SetSeg } from "../primitives";
import { UsageGroupHead } from "./UsageGroupHead";

type Mode = "model" | "day";

const MODES: { value: Mode; label: string }[] = [
  { value: "model", label: "Model" },
  { value: "day", label: "Day" },
];

/** A cost cell: the figure, the muted "Unpriced" that stands in for a model the
 *  catalog has no rate for ($0.00 would claim those calls were free), or a
 *  "≥ $x" floor for a day that mixed priced and unpriced models. */
function Cost({ label }: { label: CostLabel }) {
  const muted = label.kind === "unpriced" ? " usg-unpriced" : "";
  return (
    <span
      className={`usg-cell mono${muted}${label.tip ? " tip" : ""}`}
      data-tip={label.tip ?? undefined}
    >
      {label.text}
    </span>
  );
}

/** Placeholder rows for the first load, so the table holds its shape instead of
 *  popping in under the chart once the scan lands. */
function SkeletonRows({ count = 4 }: { count?: number }) {
  return (
    <>
      {Array.from({ length: count }, (_, i) => (
        // biome-ignore lint/suspicious/noArrayIndexKey: placeholders have no identity
        <div key={i} className="usg-row skel" aria-hidden="true">
          <Skeleton height={14} className="usg-skel-label" />
          <Skeleton height={14} />
          <Skeleton height={14} />
          <Skeleton height={14} />
        </div>
      ))}
    </>
  );
}

/** Where the window's tokens actually went, by model or by day. */
export function UsageBreakdown({ stats, loading }: { stats: UsageStats | null; loading: boolean }) {
  const [mode, setMode] = useState<Mode>("model");
  const rows = mode === "model" ? (stats?.byModel ?? []) : (stats?.byDay ?? []);
  const busy = loading && !stats;

  return (
    <section className="set-group">
      <UsageGroupHead label="Breakdown">
        <SetSeg<Mode> value={mode} options={MODES} onChange={setMode} />
      </UsageGroupHead>

      <div className="usg-table">
        <div className="usg-row head mono text-xs">
          <span>{mode === "model" ? "Model" : "Day"}</span>
          <span className="usg-cell">Cost</span>
          <span className="usg-cell">Share</span>
          <span className="usg-cell">Tokens</span>
        </div>
        {busy && <SkeletonRows />}
        {!busy && rows.length === 0 && (
          <div className="usg-row empty text-sm">Nothing to break down.</div>
        )}
        {mode === "model" &&
          stats?.byModel.map((m) => (
            <div key={`${m.provider}/${m.model}`} className="usg-row text-sm">
              <span className="usg-model flex-center">
                <ProviderIcon slug={m.provider} {...providerChip(m.provider)} size={18} />
                <span className="usg-model-id mono">{m.model}</span>
              </span>
              <Cost label={modelCostLabel(m)} />
              <span className="usg-cell usg-share mono">{formatPercent(m.share)}</span>
              <span className="usg-cell mono">{formatTokens(m.tokens)}</span>
            </div>
          ))}
        {mode === "day" &&
          stats?.byDay.map((d) => (
            <div key={d.day} className="usg-row text-sm">
              <span className="usg-day">{formatHeatDay(d.day)}</span>
              <Cost label={coverageLabel(d)} />
              <span className="usg-cell usg-share mono">{formatPercent(d.share)}</span>
              <span className="usg-cell mono">{formatTokens(d.tokens)}</span>
            </div>
          ))}
      </div>
    </section>
  );
}
