import { useState } from "react";
import type { SetupRow } from "@/components/RunConfig";
import { useAppStore } from "@/store";
import { AgentsPane } from "./AgentsPane";
import { GeneralPane } from "./GeneralPane";
import { NetworkPane } from "./NetworkPane";
import { PROJECT_SETTINGS_NAV } from "./nav";
import { RunPane } from "./RunPane";
import { SandboxPane } from "./SandboxPane";

interface Props {
  projectId: string;
  repoPath: string;
  name: string;
  rows: SetupRow[];
  ecosystem: string | null;
  overrides: Record<string, string>;
}

/** The project page's Settings tab: a row of sub-tabs under the page header,
 *  then one pane built from the global screen's rows. The frame is the
 *  project page's own (`proj-set-*`), never the global screen's rail. */
export function ProjectSettings({ projectId, repoPath, name, rows, ecosystem, overrides }: Props) {
  const section = useAppStore((s) => s.projectSettingsSection);
  const setSection = useAppStore((s) => s.setProjectSettingsSection);
  // Held here, not in the Run pane, so its edits survive a trip to another pane.
  const [runOverrides, setRunOverrides] = useState(overrides);

  return (
    <>
      <nav className="proj-set-tabs">
        {PROJECT_SETTINGS_NAV.map((n) => (
          <button
            key={n.id}
            type="button"
            className={`ps-tab iflex-center text-sm ${section === n.id ? "active" : ""}`}
            onClick={() => setSection(n.id)}
          >
            {n.label}
          </button>
        ))}
      </nav>

      {/* Keyed so each pane opens at its top. */}
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
    </>
  );
}
