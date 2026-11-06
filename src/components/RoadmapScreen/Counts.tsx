import { CountUp, Stat } from "@/components/Stats";
import type { RoadmapState } from "./Roadmap";

/** The board's counts at the header's right. Horizon counts, so they speak the
 *  horizons' planning language (see `Roadmap/types.ts`) — "in flight" is a
 *  pipeline claim horizon does not make, and the board's own rail already owns
 *  that word. */
export function Counts({ roadmap }: { roadmap: RoadmapState }) {
  const { counts, shipped } = roadmap;
  return (
    <div className="stat-row ps-stats text-sm">
      <Stat label="now">
        <CountUp value={counts.now} />
      </Stat>
      {/* Dropped first on a narrow window — see `.ps-stats-mid`. The wrapper
          is `display: contents`, so at full width the strip lays out exactly
          as if these were direct children. */}
      <span className="ps-stats-mid">
        <span className="stat-sep" />
        <Stat label="next">
          <CountUp value={counts.next} />
        </Stat>
        <span className="stat-sep" />
        <Stat label="later">
          <CountUp value={counts.later} />
        </Stat>
      </span>
      <span className="stat-sep" />
      <Stat label="shipped">
        <span className="ps-stat-shipped">
          <CountUp value={shipped} />
        </span>
      </Stat>
    </div>
  );
}
