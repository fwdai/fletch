import { useEffect, useState } from "react";
import { CountUp, formatDayTick, type MiniBar, MiniBars, Stat } from "@/components/Stats";
import { formatDuration } from "@/util/format";
import { loadMergeStats } from "./activityData";
import type { MergeStats } from "./derive";

const WEEKS = 12;

/** How long this project's pull requests take to land, and how many it opens
 *  and lands week to week.
 *
 *  The one metric on this page whose history is complete: `worktree_prs` is
 *  append-only and migration 0025 back-seeded it from the existing bindings,
 *  so these numbers cover the project rather than the time since the tab was
 *  first opened. */
export function MergeTime({ projectId }: { projectId: string }) {
  const [stats, setStats] = useState<MergeStats | null>(null);

  useEffect(() => {
    let cancelled = false;
    setStats(null);
    loadMergeStats(projectId, Date.now(), WEEKS)
      .then((s) => !cancelled && setStats(s))
      .catch((err) => console.error("merge stats failed", err));
    return () => {
      cancelled = true;
    };
  }, [projectId]);

  const bars: MiniBar[] = (stats?.weeks ?? []).map((w, i) => ({
    key: w.start,
    // Merged sits inside opened: the gap between the two bars is the week's
    // work that hasn't landed, which is the thing worth seeing at a glance.
    value: w.merged,
    backdrop: w.opened,
    // Only every third week gets a tick — 12 dates in a row is a wall of text.
    label: i % 3 === 0 ? formatDayTick(w.start) : "",
    tip: `Week of ${formatDayTick(w.start)} · ${w.opened} opened · ${w.merged} merged`,
  }));

  return (
    <section className="ps-section">
      <header className="ps-section-h">
        <h2 className="ps-section-t text-lg">Time to merge</h2>
        <p className="ps-section-lead text-sm">
          How long a pull request from this project waits between opening and landing, over its
          whole history. The bars are the last {WEEKS} weeks: opened behind, merged in front.
        </p>
      </header>

      {/* Keyed on the whole window being empty, not on `merged === 0`: a
          project that has opened PRs but landed none yet still has a real
          trend to show, and it is the one most worth looking at. */}
      <MiniBars
        bars={bars}
        loading={!stats}
        ariaLabel={`Pull requests opened and merged per week, last ${WEEKS} weeks`}
        empty={`No pull requests in the last ${WEEKS} weeks.`}
        footer={
          <span className="act-legend">
            <span className="act-key back" /> opened
            <span className="act-key" /> merged
          </span>
        }
      />

      <div className="stat-row text-sm">
        <Stat
          label="median"
          loading={!stats}
          tip={stats?.fastestMs != null ? `fastest ${formatDuration(stats.fastestMs)}` : undefined}
        >
          {stats && (stats.medianMs == null ? "—" : formatDuration(stats.medianMs))}
        </Stat>
        <span className="stat-sep" />
        <Stat label="merged" loading={!stats}>
          {stats && <CountUp value={stats.merged} />}
        </Stat>
        <span className="stat-sep" />
        <Stat label="still open" loading={!stats}>
          {stats && <CountUp value={stats.open} />}
        </Stat>
      </div>
    </section>
  );
}
