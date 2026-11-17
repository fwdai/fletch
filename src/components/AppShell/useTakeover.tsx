import type { ReactNode } from "react";
import { ProjectSettingsScreen } from "@/components/ProjectSettingsScreen";
import { RoadmapScreen } from "@/components/RoadmapScreen";
import { SettingsScreen } from "@/components/SettingsScreen";
import { UsageScreen } from "@/components/UsageScreen";
import { useAppStore, useOnScreen } from "@/store";
import { OnScreenScope } from "@/store/EngineScope";
import { projectView } from "@/store/surface";

/** The full-window screen that replaces the panes (and the sidebar), or null.
 *  Settings and Usage are This Mac's; a project's screen is the host on
 *  screen's, rendered in its scope. */
export function useTakeover(): ReactNode {
  const settingsScreenOpen = useAppStore((s) => s.settingsScreenOpen);
  const usageScreenOpen = useAppStore((s) => s.usageScreenOpen);
  const projectRepoPath = useOnScreen((s) => s.projectScreen?.repoPath ?? null);
  const view = useOnScreen(projectView);

  if (settingsScreenOpen) return <SettingsScreen />;
  if (usageScreenOpen) return <UsageScreen />;
  if (projectRepoPath && view === "settings") {
    return (
      <OnScreenScope>
        <ProjectSettingsScreen repoPath={projectRepoPath} />
      </OnScreenScope>
    );
  }
  if (projectRepoPath && view === "roadmap") {
    return (
      <OnScreenScope>
        <RoadmapScreen repoPath={projectRepoPath} />
      </OnScreenScope>
    );
  }
  return null;
}
