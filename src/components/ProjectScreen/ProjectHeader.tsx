import { Fragment } from "react";
import { Icon, type IconName } from "@/components/Icon";
import { ProjectIcon } from "@/components/ProjectIcon";
import { CountUp, Stat } from "@/components/Stats";
import { IconButton } from "@/components/ui/IconButton";
import { useAppStore } from "@/store";
import type { ProjectScreenTab } from "@/store/ui";
import type { RoadmapState } from "./Roadmap";

const TABS: { id: ProjectScreenTab; label: string; icon: IconName }[] = [
  { id: "roadmap", label: "Roadmap", icon: "map" },
  { id: "activity", label: "Activity", icon: "activity" },
  { id: "context", label: "Context", icon: "graph" },
  { id: "settings", label: "Settings", icon: "settings" },
];

/** The project page header: one thin chrome bar — back and the project's
 *  identity on the left, the tab segment centered, the board counts on the
 *  right. The identity sits in the back button's old zone, so every tab says
 *  which project it is without a second row. */
export function ProjectHeader({
  name,
  roadmap,
  tab,
  onTab,
  onClose,
}: {
  name: string;
  roadmap: RoadmapState;
  tab: ProjectScreenTab;
  onTab: (tab: ProjectScreenTab) => void;
  onClose: () => void;
}) {
  const { counts, shipped } = roadmap;
  // The Context tab exists only while the layer's developer gate is open.
  const contextLayerEnabled = useAppStore((s) => s.contextLayerEnabled);
  const tabs = TABS.filter((t) => t.id !== "context" || contextLayerEnabled);

  return (
    <div className="ps-head">
      <div className="ps-ident flex-center">
        <IconButton tip="Back to app" onClick={onClose}>
          <Icon name="chevL" size={13} />
        </IconButton>
        <ProjectIcon name={name} />
        <span className="ps-name text-base truncate">{name}</span>
      </div>

      <nav className="ps-tabs">
        {tabs.map((t) => (
          <Fragment key={t.id}>
            {/* The feature tabs and the configuration tab are two things. */}
            {t.id === "settings" && <span className="ps-tab-sep" />}
            <button
              type="button"
              className={`ps-tab iflex-center text-sm ${tab === t.id ? "active" : ""}`}
              onClick={() => onTab(t.id)}
            >
              <Icon name={t.icon} size={13} />
              {t.label}
            </button>
          </Fragment>
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
