import { Suspense, lazy } from "react";
import { useAppStore, type SettingsSection } from "../../store";
import { Icon, type IconName } from "../Icon";
import pkg from "../../../package.json";
import { GeneralPane } from "./GeneralPane";
import { AccountPane } from "./AccountPane";
import { ProvidersPane } from "./ProvidersPane";
import { ExperimentalPane } from "./ExperimentalPane";

// Lazily loaded behind `import.meta.env.DEV`. In production the ternary's dead
// branch — including the dynamic import() — is dropped by Rollup, so the
// DeveloperPane chunk is never emitted into the build (not merely unloaded).
const DeveloperPane = import.meta.env.DEV
  ? lazy(() =>
      import("./DeveloperPane").then((m) => ({ default: m.DeveloperPane })),
    )
  : null;

const NAV: { id: SettingsSection; label: string; icon: IconName }[] = [
  { id: "account", label: "Account", icon: "user" },
  { id: "general", label: "General", icon: "settings" },
  { id: "providers", label: "Providers", icon: "cube" },
  { id: "experimental", label: "Experimental", icon: "flask" },
  // Dev-only: omitted entirely from production builds.
  ...(DeveloperPane
    ? [{ id: "developer" as const, label: "Developer", icon: "wrench" as const }]
    : []),
];

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
        <button className="set-back" onClick={close}>
          <Icon name="chevL" size={13} />
          <span>Back to app</span>
        </button>
        <div className="set-nav-list">
          {NAV.map((n) => (
            <button
              key={n.id}
              className={`set-nav-item ${section === n.id ? "active" : ""}`}
              onClick={() => setSection(n.id)}
            >
              <Icon name={n.icon} size={14} />
              <span>{n.label}</span>
            </button>
          ))}
        </div>
        <div className="set-nav-foot">
          <span className="mono">Quorum</span>
          <span className="mono dim">v{pkg.version}</span>
        </div>
      </nav>

      <div className="set-main">
        <div className="set-content">
          {section === "account" && <AccountPane />}
          {section === "providers" && <ProvidersPane />}
          {section === "experimental" && <ExperimentalPane />}
          {section === "developer" && DeveloperPane && (
            <Suspense fallback={null}>
              <DeveloperPane />
            </Suspense>
          )}
          {/* Fallback: "developer" can't be selected in prod (no nav entry, and
              DeveloperPane is null), so a stale section value falls back here. */}
          {(section === "general" ||
            (section === "developer" && !DeveloperPane)) && <GeneralPane />}
        </div>
      </div>
    </div>
  );
}
