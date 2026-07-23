import { lazy, Suspense, useMemo } from "react";
import { Icon, type IconName } from "@/components/Icon";
import type { SettingsSection } from "@/storage/preferences";
import { useAppStore } from "@/store";
import { WorkflowsPane } from "@/workflows/builder";
import pkg from "../../../package.json";
import { AccountPane } from "./AccountPane";
import { CustomAgentsPane } from "./CustomAgents";
import { ExperimentalPane } from "./ExperimentalPane";
import { GeneralPane } from "./GeneralPane";
import { McpServersPane } from "./McpServers";
import { ProvidersPane } from "./ProvidersPane";
import { SkillsPane } from "./Skills";

// Lazily loaded — code-split into its own chunk, fetched only when the Developer
// section is actually opened. Visibility is gated at render (dev builds, or an
// admin user in production), not by dropping the chunk from the build.
const DeveloperPane = lazy(() =>
  import("./DeveloperPane").then((m) => ({ default: m.DeveloperPane })),
);

// Built-in sections carry explicit order weights (spaced by 10) so extension
// panes can slot *between* them via their own `order`, not just append.
// `subsections` groups several sections behind a single nav entry, surfaced as
// an in-pane segmented switch (see CUSTOMIZE_IDS below).
type NavItem = {
  id: SettingsSection;
  label: string;
  icon: IconName;
  order: number;
  subsections?: SettingsSection[];
};

// Custom agents / Tools / Skills are all agent-building primitives — tools and
// skills exist mainly to be composed into agents — so they live behind one
// "Customize" nav entry with an in-pane segmented switch (see CustomizeSwitch).
// The nav entry's `id` is the default sub-tab.
const CUSTOMIZE_IDS: SettingsSection[] = ["agents", "tools", "skills"];

const NAV: NavItem[] = [
  { id: "account", label: "Account", icon: "user", order: 10 },
  { id: "general", label: "General", icon: "settings", order: 20 },
  { id: "providers", label: "Providers", icon: "cube", order: 30 },
  {
    id: "agents",
    label: "Customize",
    icon: "sparkle",
    order: 40,
    subsections: CUSTOMIZE_IDS,
  },
  // Right after Customize — workflows chain those custom agents.
  { id: "workflows", label: "Workflows", icon: "combine", order: 41 },
  { id: "experimental", label: "Experimental", icon: "flask", order: 50 },
];
// Stable sort by weight keeps contribution order on ties.
NAV.sort((a, b) => a.order - b.order);

// Developer is appended at render only when unlocked (dev build or admin user),
// so it slots by `order` among the base entries above.
const DEVELOPER_NAV: NavItem = { id: "developer", label: "Developer", icon: "wrench", order: 60 };

/** Dedicated full-screen settings surface. Rendered in place of the workspace
 *  panes while `settingsScreenOpen` is true. The quick-settings popover stays
 *  for fast access; this is the comprehensive surface. */
export function SettingsScreen() {
  const section = useAppStore((s) => s.settingsSection);
  const setSection = useAppStore((s) => s.setSettingsSection);
  const close = useAppStore((s) => s.closeSettingsScreen);
  const admin = useAppStore((s) => s.admin);

  // Dev builds always expose Developer; production unlocks it only for admins.
  const showDeveloper = import.meta.env.DEV || admin;
  const nav = useMemo(
    () => (showDeveloper ? [...NAV, DEVELOPER_NAV].sort((a, b) => a.order - b.order) : NAV),
    [showDeveloper],
  );

  return (
    <div className="set-screen">
      <nav className="set-nav">
        <button className="set-back flex-center text-base" onClick={close}>
          <Icon name="chevL" size={13} />
          <span>Back to app</span>
        </button>
        <div className="set-nav-list">
          {nav.map((n) => {
            // A grouped entry stays active for any of its sub-sections, and
            // clicking it while already inside the group keeps the current
            // sub-tab rather than snapping back to the default.
            const active = n.subsections ? n.subsections.includes(section) : section === n.id;
            return (
              <button
                key={n.id}
                className={`set-nav-item flex-center text-base ${active ? "active" : ""}`}
                onClick={() => {
                  if (active && n.subsections) return;
                  setSection(n.id);
                }}
              >
                <Icon name={n.icon} size={14} />
                <span>{n.label}</span>
              </button>
            );
          })}
        </div>
        <div className="set-nav-foot text-xs">
          <span className="mono">Fletch</span>
          <span className="mono dim">v{pkg.version}</span>
        </div>
      </nav>

      <div className="set-main">
        <div className="set-content">
          {section === "account" && <AccountPane />}
          {section === "providers" && <ProvidersPane />}
          {section === "agents" && <CustomAgentsPane />}
          {section === "workflows" && <WorkflowsPane />}
          {section === "skills" && <SkillsPane />}
          {section === "tools" && <McpServersPane />}
          {section === "experimental" && <ExperimentalPane />}
          {section === "developer" && showDeveloper && (
            <Suspense fallback={null}>
              <DeveloperPane />
            </Suspense>
          )}
          {/* Fallback: a stale "developer" section value (e.g. an admin flag
              that flipped off) has no nav entry, so it falls back to General. */}
          {(section === "general" || (section === "developer" && !showDeveloper)) && (
            <GeneralPane />
          )}
        </div>
      </div>
    </div>
  );
}
