import type { IconName } from "@/components/Icon";
import type { ProjectSettingsSection } from "@/store/ui";

/** The settings view's tabs, in display order — the header's segmented control. */
export const PROJECT_SETTINGS_NAV: { id: ProjectSettingsSection; label: string; icon: IconName }[] =
  [
    { id: "general", label: "General", icon: "folder" },
    { id: "run", label: "Run", icon: "play" },
    { id: "sandbox", label: "Sandbox", icon: "cube" },
    { id: "network", label: "Network", icon: "globe" },
    { id: "agents", label: "Agents", icon: "sparkle" },
  ];
