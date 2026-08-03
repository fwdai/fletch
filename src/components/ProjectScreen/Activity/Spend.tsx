import { useEffect, useMemo, useState } from "react";
import { formatDayTick, formatHeatDay, type MiniBar, MiniBars, Stat } from "@/components/Stats";
import { formatCost, formatTokens } from "@/util/format";
import { loadSpend } from "./activityData";
import { recentDays, type SpendDay } from "./derive";

const DAYS = 30;

/** What this project has been spending, day by day.
 *
 *  Plots tokens rather than dollars because tokens are the one unit every
 *  provider reports — claude, codex and cursor price nothing, so a cost chart
 *  would be empty for most projects. Cost rides along in the tooltip and the
 *  stat row wherever a provider actually reported it. */
export function Spend({ projectId }: { projectId: string }) {
  const [spend, setSpend] = useState<SpendDay[] | null>(null);
  const days = useMemo(() => recentDays(Date.now(), DAYS), []);

  useEffect(() => {
    let cancelled = false;
    setSpend(null);
    loadSpend(projectId, days)
      .then((s) => !cancelled && setSpend(s))
      .catch((err) => console.error("spend series failed", err));
    return () => {
      cancelled = true;
    };
  }, [projectId, days]);

  const bars: MiniBar[] = (spend ?? []).map((d, i) => ({
    key: d.day,
    value: d.tokens,
    label: i % 7 === 0 ? formatDayTick(d.day) : "",
    tip:
      d.tokens == null
        ? `${formatHeatDay(d.day)} · not recorded`
        : `${formatHeatDay(d.day)} · ${formatTokens(d.tokens)} tokens${
            d.cost != null && d.cost > 0 ? ` · ${formatCost(d.cost)}` : ""
          }`,
  }));

  // Only over days that were actually observed — summing nulls as zero would
  // present a partial window as a complete one.
  const observed = (spend ?? []).filter((d) => d.tokens != null);
  const tokens = observed.reduce((n, d) => n + (d.tokens ?? 0), 0);
  const cost = observed.reduce((n, d) => n + (d.cost ?? 0), 0);

  return (
    <section className="ps-section">
      <header className="ps-section-h">
        <h2 className="ps-section-t text-lg">Spend over time</h2>
        <p className="ps-section-lead text-sm">
          Fresh input and output tokens per day for the last {DAYS} days. History accrues only while
          you use the app, so days before this project’s first visit here — and any day it went
          unopened — show as a gap rather than a zero.
        </p>
      </header>

      <MiniBars
        bars={bars}
        loading={!spend}
        ariaLabel={`Tokens used per day, last ${DAYS} days`}
        empty={
          <>
            No usage recorded yet. This starts filling in from the next time an agent takes a turn
            on this project.
          </>
        }
      />

      <div className="stat-row text-sm">
        <Stat
          label="tokens"
          loading={!spend}
          tip={observed.length > 0 ? `over ${observed.length} recorded days` : undefined}
        >
          {spend && formatTokens(tokens)}
        </Stat>
        {cost > 0 && (
          <>
            <span className="stat-sep" />
            <Stat label="cost" loading={!spend} tip="only providers that price their own calls">
              {spend && formatCost(cost)}
            </Stat>
          </>
        )}
      </div>
    </section>
  );
}
