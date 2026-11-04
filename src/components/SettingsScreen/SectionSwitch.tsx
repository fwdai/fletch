import { Icon, type IconName } from "@/components/Icon";
import type { SettingsSection } from "@/storage/preferences";
import { useAppStore } from "@/store";

export interface SectionTab {
  id: SettingsSection;
  label: string;
  icon: IconName;
}

/** The switch between the sections behind one nav entry, shown on the eyebrow
 *  row of each of their panes, in the shared `.set-seg` pill styling. Reads the
 *  active section straight from the store so a pane can drop it in without
 *  threading section state through. */
export function SectionSwitch({ tabs }: { tabs: SectionTab[] }) {
  const section = useAppStore((s) => s.settingsSection);
  const setSection = useAppStore((s) => s.setSettingsSection);

  return (
    <div className="set-seg">
      {tabs.map((t) => (
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
