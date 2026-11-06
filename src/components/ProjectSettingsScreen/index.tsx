import { useState } from "react";
import { ProjectHeader, ViewTabs } from "@/components/ProjectScreen/Header";
import { Loader } from "@/components/ui/Loader";
import { useAppStore } from "@/store";
import type { ProjectSettingsSection } from "@/store/ui";
import { basename } from "@/util/format";
import { AgentsPane } from "./AgentsPane";
import { GeneralPane } from "./GeneralPane";
import { NetworkPane } from "./NetworkPane";
import { PROJECT_SETTINGS_NAV } from "./nav";
import { RunPane } from "./RunPane";
import { SandboxPane } from "./SandboxPane";
import { type RunConfig, useRunConfig } from "./useRunConfig";

/** A project's settings, full screen: the per-project config every agent in
 *  the project inherits. Resolves the project_id and the detected run config
 *  on open; the panes mount once both are in. */
export function ProjectSettingsScreen({ repoPath }: { repoPath: string }) {
  const { loaded, error } = useRunConfig(repoPath);
  const name = useAppStore(
    (s) => s.workspace?.projects.find((p) => p.path === repoPath)?.name ?? basename(repoPath),
  );
  const section = useAppStore((s) => s.projectSettingsSection);
  const setSection = useAppStore((s) => s.setProjectSettingsSection);

  return (
    <div className="proj-screen">
      <ProjectHeader repoPath={repoPath} view="settings">
        <ViewTabs tabs={PROJECT_SETTINGS_NAV} value={section} onChange={setSection} />
      </ProjectHeader>
      {loaded ? (
        <ProjectSettings {...loaded} repoPath={repoPath} name={name} section={section} />
      ) : (
        <div className="ps-content">
          <div className="ps-sections">
            {error ? (
              <div className="ps-state text-sm">Couldn’t load this project.</div>
            ) : (
              <div className="ps-state iflex-center text-sm">
                <Loader variant="inherit" /> Loading…
              </div>
            )}
          </div>
        </div>
      )}
    </div>
  );
}

interface Props extends RunConfig {
  repoPath: string;
  name: string;
  section: ProjectSettingsSection;
}

/** One pane built from the global screen's rows, under the header's tabs. The
 *  frame is the project's own (`ps-*`), never the global screen's rail. */
function ProjectSettings({
  projectId,
  repoPath,
  name,
  section,
  rows,
  ecosystem,
  overrides,
}: Props) {
  // Held here, not in the Run pane, so its edits survive a trip to another pane.
  const [runOverrides, setRunOverrides] = useState(overrides);

  return (
    // Keyed so each pane opens at its top.
    <div key={section} className="ps-content">
      <div className="ps-sections">
        {section === "general" && <GeneralPane projectId={projectId} name={name} />}
        {section === "run" && (
          <RunPane
            projectId={projectId}
            repoPath={repoPath}
            rows={rows}
            ecosystem={ecosystem}
            overrides={runOverrides}
            onOverrides={setRunOverrides}
          />
        )}
        {section === "sandbox" && <SandboxPane projectId={projectId} repoPath={repoPath} />}
        {section === "network" && <NetworkPane projectId={projectId} />}
        {section === "agents" && <AgentsPane projectId={projectId} />}
      </div>
    </div>
  );
}
