import type { ProjectSettingsSection } from "@/store/ui";

/** The settings view's sub-tabs, in display order. */
export const PROJECT_SETTINGS_NAV: { id: ProjectSettingsSection; label: string }[] = [
  { id: "general", label: "General" },
  { id: "run", label: "Run" },
  { id: "sandbox", label: "Sandbox" },
  { id: "network", label: "Network" },
  { id: "agents", label: "Agents" },
];
