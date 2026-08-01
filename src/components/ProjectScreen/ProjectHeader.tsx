import { Icon, type IconName } from "@/components/Icon";
import { CountUp, Stat } from "@/components/Stats";
import type { ProjectScreenTab } from "@/store/ui";
import type { RoadmapState } from "./Roadmap";

const TABS: { id: ProjectScreenTab; label: string; icon: IconName }[] = [
  { id: "roadmap", label: "Roadmap", icon: "map" },
  { id: "settings", label: "Settings", icon: "settings" },
];

/** The project page header: the back button, the project's identity, the board
 *  counts right-aligned, and the tab row sitting on the bottom rule. */
export function ProjectHeader({
  name,
  subtitle,
  roadmap,
  tab,
  onTab,
  onClose,
}: {
  name: string;
  /** Repo path, or the repo count for a multi-repo project. */
  subtitle: string;
  roadmap: RoadmapState;
  tab: ProjectScreenTab;
  onTab: (tab: ProjectScreenTab) => void;
  onClose: () => void;
}) {
  const { counts, shipped } = roadmap;

  return (
    <div className="ps-head">
      <div className="ps-head-row">
        <button type="button" className="ps-back flex-center text-base" onClick={onClose}>
          <Icon name="chevL" size={13} />
          <span>Back to app</span>
        </button>
        <div className="ps-id">
          <div className="ps-title text-lg truncate">{name}</div>
          <div className="ps-path mono text-xs truncate">{subtitle}</div>
        </div>
        <div className="stat-row ps-stats text-sm">
          <Stat label="in flight">
            <CountUp value={counts.now} />
          </Stat>
          <span className="stat-sep" />
          <Stat label="next">
            <CountUp value={counts.next} />
          </Stat>
          <span className="stat-sep" />
          <Stat label="later">
            <CountUp value={counts.later} />
          </Stat>
          <span className="stat-sep" />
          <Stat label="shipped">
            <span className="ps-stat-shipped">
              <CountUp value={shipped} />
            </span>
          </Stat>
        </div>
      </div>

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
    </div>
  );
}
