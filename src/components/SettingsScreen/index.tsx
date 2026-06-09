import { useAppStore, type SettingsSection } from "../../store";
import { Icon, type IconName } from "../Icon";
import pkg from "../../../package.json";
import { GeneralPane } from "./GeneralPane";
import { AccountPane } from "./AccountPane";
import { ProvidersPane } from "./ProvidersPane";
import { DeveloperPane } from "./DeveloperPane";

const NAV: { id: SettingsSection; label: string; icon: IconName }[] = [
  { id: "account", label: "Account", icon: "user" },
  { id: "general", label: "General", icon: "settings" },
  { id: "providers", label: "Providers", icon: "cube" },
  // Dev-only: omitted entirely from production builds.
  ...(import.meta.env.DEV
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
          {section === "developer" && import.meta.env.DEV && <DeveloperPane />}
          {/* Fallback: "developer" can't be selected in prod (no nav entry), but
              guard the render so a stale section value still shows something. */}
          {(section === "general" ||
            (section === "developer" && !import.meta.env.DEV)) && <GeneralPane />}
        </div>
      </div>
    </div>
  );
}
