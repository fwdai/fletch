import { useEffect, useState } from "react";
import {
  ActivityHeatmap,
  CountUp,
  computeStreak,
  formatHeatDay,
  Stat,
} from "@/components/Stats";
import { formatCost, formatTokens } from "@/util/format";
import {
  loadPulseActivity,
  loadPulseTotals,
  loadPulseUsage,
  type PulseActivity,
  type PulseTotals,
  type PulseUsage,
} from "./pulseData";

const WEEKS = 52;
// One extra week of margin past the grid so the oldest column is never short.
const HORIZON_MS = (WEEKS + 1) * 7 * 86_400_000;

/** The "Project Pulse" block atop Project Settings: a year of daily activity
 *  on the accent heat ramp, a streak counter, and lifetime hero numbers.
 *  Turn/agent/PR series load in one round of fast indexed queries; the token
 *  total folds every session's records, so it streams in behind a shimmer. */
export function ProjectPulse({ projectId }: { projectId: string }) {
  const [activity, setActivity] = useState<PulseActivity | null>(null);
  const [totals, setTotals] = useState<PulseTotals | null>(null);
  const [usage, setUsage] = useState<PulseUsage | null>(null);

  useEffect(() => {
    let cancelled = false;
    setActivity(null);
    setTotals(null);
    setUsage(null);
    const now = Date.now();
    loadPulseActivity(projectId, now - HORIZON_MS)
      .then((a) => !cancelled && setActivity(a))
      .catch((err) => console.error("pulse activity failed", err));
    loadPulseTotals(projectId, now)
      .then((t) => !cancelled && setTotals(t))
      .catch((err) => console.error("pulse totals failed", err));
    loadPulseUsage(projectId)
      .then((u) => !cancelled && setUsage(u))
      .catch((err) => {
        console.error("pulse usage failed", err);
        if (!cancelled) setUsage({ tokens: 0, costUsd: 0 });
      });
    return () => {
      cancelled = true;
    };
  }, [projectId]);

  const turns = activity?.turns ?? {};
  const streak = activity ? computeStreak(turns, Date.now()) : 0;

  // Date + at most two metrics — more reads as clutter. Turns lead (they're
  // the cell's intensity); PRs beat agents for the second slot because
  // they're the rarer, more notable signal.
  const tooltipFor = (day: string, count: number) => {
    const parts: string[] = [];
    if (count > 0) parts.push(`${count} turn${count === 1 ? "" : "s"}`);
    const prs = activity?.prs[day] ?? 0;
    if (prs > 0) parts.push(`${prs} PR${prs === 1 ? "" : "s"}`);
    const agents = activity?.agents[day] ?? 0;
    if (agents > 0) parts.push(`${agents} agent${agents === 1 ? "" : "s"}`);
    if (parts.length === 0) parts.push("no activity");
    return `${formatHeatDay(day)} · ${parts.slice(0, 2).join(" · ")}`;
  };

  return (
    <section className="ps-section">
      {activity ? (
        <ActivityHeatmap
          counts={turns}
          weeks={WEEKS}
          tooltipFor={tooltipFor}
          footer={
            streak > 1 && (
              <div className="pulse-streak iflex-center">
                <span className="pulse-streak-dot" />
                {streak}-day streak
              </div>
            )
          }
        />
      ) : (
        <div className="pulse-skeleton" />
      )}

      <div className="stat-row text-sm">
        <Stat
          label="agents"
          loading={!totals}
          tip={
            totals && totals.agents7d > 0 ? `${totals.agents7d} in the last 7 days` : undefined
          }
        >
          {totals && <CountUp value={totals.agents} />}
        </Stat>
        <span className="stat-sep" />
        <Stat
          label="PRs"
          loading={!totals}
          tip={totals && totals.prsMerged > 0 ? `${totals.prsMerged} merged` : undefined}
        >
          {totals && <CountUp value={totals.prsOpened} />}
        </Stat>
        <span className="stat-sep" />
        <Stat label="lines" loading={!totals}>
          {totals && (
            <>
              <span className="stat-add">
                +<CountUp value={totals.additions} />
              </span>
              <span className="stat-del">
                −<CountUp value={totals.deletions} />
              </span>
            </>
          )}
        </Stat>
        <span className="stat-sep" />
        <Stat
          label="tokens"
          loading={!usage}
          tip={usage && usage.costUsd > 0 ? `≈ ${formatCost(usage.costUsd)}` : undefined}
        >
          {usage && <CountUp value={usage.tokens} format={formatTokens} />}
        </Stat>
      </div>
    </section>
  );
}
