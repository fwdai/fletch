import { lazy, Suspense } from "react";
import { Icon, type IconName } from "@/components/Icon";
import type { SettingsSection } from "@/storage/preferences";
import { useAppStore } from "@/store";
import pkg from "../../../package.json";
import { AccountPane } from "./AccountPane";
import { CustomAgentsPane } from "./CustomAgents";
import { ExperimentalPane } from "./ExperimentalPane";
import { GeneralPane } from "./GeneralPane";
import { McpServersPane } from "./McpServers";
import { ProvidersPane } from "./ProvidersPane";
import { SkillsPane } from "./Skills";

// Lazily loaded behind `import.meta.env.DEV`. In production the ternary's dead
// branch — including the dynamic import() — is dropped by Rollup, so the
// DeveloperPane chunk is never emitted into the build (not merely unloaded).
const DeveloperPane = import.meta.env.DEV
  ? lazy(() => import("./DeveloperPane").then((m) => ({ default: m.DeveloperPane })))
  : null;

// Built-in sections carry explicit order weights (spaced by 10) so extension
// panes can slot *between* them via their own `order`, not just append.
type NavItem = { id: SettingsSection; label: string; icon: IconName; order: number };

const NAV: NavItem[] = [
  { id: "account", label: "Account", icon: "user", order: 10 },
  { id: "general", label: "General", icon: "settings", order: 20 },
  { id: "providers", label: "Providers", icon: "cube", order: 30 },
  { id: "agents", label: "Custom agents", icon: "bot", order: 40 },
  { id: "skills", label: "Skills", icon: "notebookPen", order: 42 },
  { id: "tools", label: "Tools", icon: "zap", order: 44 },
  { id: "experimental", label: "Experimental", icon: "flask", order: 50 },
  // Dev-only: omitted entirely from production builds.
  ...(DeveloperPane
    ? [{ id: "developer" as const, label: "Developer", icon: "wrench" as const, order: 60 }]
    : []),
];
// Stable sort by weight keeps contribution order on ties.
NAV.sort((a, b) => a.order - b.order);

/** Dedicated full-screen settings surface. Rendered in place of the workspace
 *  panes while `settingsScreenOpen` is true. The quick-settings popover stays
 *  for fast access; this is the comprehensive surface. */
export function SettingsScreen() {
  const section = useAppStore((s) => s.settingsSection);
  const setSection = useAppStore((s) => s.setSettingsSection);
  const close = useAppStore((s) => s.closeSettingsScreen);

  return (
    <div className="set-screen">
      <nav className="set-nav">
        <button className="set-back flex-center text-base" onClick={close}>
          <Icon name="chevL" size={13} />
          <span>Back to app</span>
        </button>
        <div className="set-nav-list">
          {NAV.map((n) => (
            <button
              key={n.id}
              className={`set-nav-item flex-center text-base ${section === n.id ? "active" : ""}`}
              onClick={() => setSection(n.id)}
            >
              <Icon name={n.icon} size={14} />
              <span>{n.label}</span>
            </button>
          ))}
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
          {section === "skills" && <SkillsPane />}
          {section === "tools" && <McpServersPane />}
          {section === "experimental" && <ExperimentalPane />}
          {section === "developer" && DeveloperPane && (
            <Suspense fallback={null}>
              <DeveloperPane />
            </Suspense>
          )}
          {/* Fallback: "developer" can't be selected in prod (no nav entry, and
              DeveloperPane is null), so a stale section value falls back here. */}
          {(section === "general" || (section === "developer" && !DeveloperPane)) && (
            <GeneralPane />
          )}
        </div>
      </div>
    </div>
  );
}
