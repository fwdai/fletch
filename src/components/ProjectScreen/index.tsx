import { useEffect, useState } from "react";
import { api } from "@/api";
import { loadRunOverrides, type SetupRow, toSetupRows } from "@/components/RunConfig";
import { Loader } from "@/components/ui/Loader";
import { useAppStore } from "@/store";
import { basename } from "@/util/format";
import { Activity } from "./Activity";
import { AutopilotSection } from "./AutopilotSection";
import { DeleteSection } from "./DeleteSection";
import { EnvVarsSection } from "./EnvVarsSection";
import { GeneralSection } from "./GeneralSection";
import { LinearSection } from "./LinearSection";
import { ProjectHeader } from "./ProjectHeader";
import { Roadmap, useRoadmap } from "./Roadmap";
import { RoadmapSection } from "./RoadmapSection";
import { RunEnvSection } from "./RunEnvSection";
import { VerifySection } from "./VerifySection";

interface Loaded {
  projectId: string;
  rows: SetupRow[];
  ecosystem: string | null;
  overrides: Record<string, string>;
}

/** Full-screen project page. Rendered in place of the workspace panes while
 *  `projectScreenRepoPath` is set (mirrors SettingsScreen). Three tabs under a
 *  shared header, one per question you can ask about a project: the Roadmap
 *  (what gets built next), Activity (what has been built), and Settings (the
 *  per-project config every agent in the project inherits).
 *
 *  Keyed by the project's primary repo path; resolves the project_id and
 *  detected run config on open. Both non-roadmap tabs need the resolved
 *  project_id, so they share one load gate. */
export function ProjectScreen({ repoPath }: { repoPath: string }) {
  const close = useAppStore((s) => s.closeProjectScreen);
  const projects = useAppStore((s) => s.workspace?.projects);
  // The tab lives in the store so whoever opened the screen picks it — the
  // sidebar's "Project settings" gear lands on Settings, the title-bar pill
  // on the roadmap.
  const tab = useAppStore((s) => s.projectScreenTab);
  const setTab = useAppStore((s) => s.setProjectScreenTab);
  const [loaded, setLoaded] = useState<Loaded | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Lives here, not in <Roadmap>, because the header shows the same counts —
  // and they move when the user accepts a proposal. Keyed by the repo, which
  // the hook resolves to the owning project (one roadmap per project).
  const roadmap = useRoadmap(repoPath);

  // Custom display name for this repo, falling back to the folder basename.
  // Not shown in the page header (the title bar already names the project) —
  // the settings sections below still need it.
  const name = projects?.find((p) => p.path === repoPath)?.name ?? basename(repoPath);

  // Resolve project_id + detected run config for the repo, then load the
  // persisted overrides. Both must be ready before the editor mounts so the
  // draft baseline is correct.
  useEffect(() => {
    let cancelled = false;
    setLoaded(null);
    setError(null);
    (async () => {
      try {
        const { project_id, configs } = await api.projectRunConfig(repoPath);
        const overrides = await loadRunOverrides(project_id);
        if (cancelled) return;
        const primary = configs[0];
        setLoaded({
          projectId: project_id,
          rows: toSetupRows(primary?.rows ?? []),
          ecosystem: primary?.ecosystem ?? null,
          overrides,
        });
      } catch (err) {
        if (cancelled) return;
        console.error("projectRunConfig failed", err);
        setError(String(err));
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [repoPath]);

  return (
    <div className="proj-screen">
      <ProjectHeader roadmap={roadmap} tab={tab} onTab={setTab} onClose={close} />

      {tab === "roadmap" ? (
        <Roadmap roadmap={roadmap} repoPath={repoPath} />
      ) : (
        <div className="ps-content">
          <div className="ps-sections">
            {error ? (
              <div className="ps-state text-sm">Couldn’t load this project.</div>
            ) : !loaded ? (
              <div className="ps-state iflex-center text-sm">
                <Loader variant="inherit" /> Loading…
              </div>
            ) : tab === "activity" ? (
              <Activity projectId={loaded.projectId} />
            ) : (
              <>
                <GeneralSection projectId={loaded.projectId} currentName={name} />
                <RunEnvSection
                  projectId={loaded.projectId}
                  rows={loaded.rows}
                  ecosystem={loaded.ecosystem}
                  initialOverrides={loaded.overrides}
                />
                <EnvVarsSection projectId={loaded.projectId} repoPath={repoPath} />
                <LinearSection projectId={loaded.projectId} />
                <RoadmapSection projectId={loaded.projectId} />
                <AutopilotSection projectId={loaded.projectId} />
                <VerifySection projectId={loaded.projectId} />
                <DeleteSection projectId={loaded.projectId} projectName={name} />
              </>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
