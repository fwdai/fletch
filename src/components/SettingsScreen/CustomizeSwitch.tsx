import { Icon, type IconName } from "@/components/Icon";
import type { SettingsSection } from "@/storage/preferences";
import { useAppStore } from "@/store";

/** The Custom agents / Tools / Skills switch shown on the eyebrow row of each
 *  Customize pane. Same `.set-seg` pill styling as the new-agent page's
 *  Agent | Workflow toggle. Reads the active section straight from the store so
 *  the list panes can drop it in without threading section state through. */
const TABS: { id: SettingsSection; label: string; icon: IconName }[] = [
  { id: "agents", label: "Custom agents", icon: "bot" },
  { id: "tools", label: "Tools", icon: "zap" },
  { id: "skills", label: "Skills", icon: "notebookPen" },
];

export function CustomizeSwitch() {
  const section = useAppStore((s) => s.settingsSection);
  const setSection = useAppStore((s) => s.setSettingsSection);

  return (
    <div className="set-seg">
      {TABS.map((t) => (
        <button
          key={t.id}
          type="button"
          className={section === t.id ? "active" : ""}
          onClick={() => setSection(t.id)}
        >
          <Icon name={t.icon} size={13} /> {t.label}
        </button>
      ))}
    </div>
  );
}
