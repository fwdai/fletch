import { Icon, type IconName } from "@/components/Icon";
import { CountUp, Stat } from "@/components/Stats";
import type { ProjectScreenTab } from "@/store/ui";
import type { RoadmapState } from "./Roadmap";

const TABS: { id: ProjectScreenTab; label: string; icon: IconName }[] = [
  { id: "roadmap", label: "Roadmap", icon: "map" },
  { id: "activity", label: "Activity", icon: "activity" },
  { id: "settings", label: "Settings", icon: "settings" },
];

/** The project page header: one thin chrome bar — back on the left, the tab
 *  segment centered, the board counts on the right.
 *
 *  No project name or path here: the title bar already names the project the
 *  page belongs to, and repeating it cost a whole second row of height above
 *  two panels that each have a header of their own. */
export function ProjectHeader({
  roadmap,
  tab,
  onTab,
  onClose,
}: {
  roadmap: RoadmapState;
  tab: ProjectScreenTab;
  onTab: (tab: ProjectScreenTab) => void;
  onClose: () => void;
}) {
  const { counts, shipped } = roadmap;

  return (
    <div className="ps-head">
      <button type="button" className="ps-back flex-center text-sm" onClick={onClose}>
        <Icon name="chevL" size={13} />
        <span>Back to app</span>
      </button>

      <nav className="ps-tabs">
        {TABS.map((t) => (
          <button
            key={t.id}
            type="button"
            className={`ps-tab iflex-center text-sm ${tab === t.id ? "active" : ""}`}
            onClick={() => onTab(t.id)}
          >
            <Icon name={t.icon} size={13} />
            {t.label}
          </button>
        ))}
      </nav>

      <div className="stat-row ps-stats text-sm">
        {/* Horizon counts, so they speak the horizons' planning language (see
            `Roadmap/types.ts`) — "in flight" is a pipeline claim horizon does not
            make, and the board's own rail already owns that word. */}
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
    </div>
  );
}
