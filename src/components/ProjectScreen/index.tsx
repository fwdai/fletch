import { useEffect, useState } from "react";
import { api } from "@/api";
import { loadRunOverrides, type SetupRow, toSetupRows } from "@/components/RunConfig";
import { Loader } from "@/components/ui/Loader";
import { useAppStore } from "@/store";
import { basename } from "@/util/format";
import { DeleteSection } from "./DeleteSection";
import { EnvVarsSection } from "./EnvVarsSection";
import { GeneralSection } from "./GeneralSection";
import { LinearSection } from "./LinearSection";
import { ProjectHeader, type ProjectTab } from "./ProjectHeader";
import { ProjectPulse } from "./ProjectPulse";
import { Roadmap, useRoadmap } from "./Roadmap";
import { RunEnvSection } from "./RunEnvSection";
import { VerifySection } from "./VerifySection";

interface Loaded {
  projectId: string;
  rows: SetupRow[];
  ecosystem: string | null;
  overrides: Record<string, string>;
}

/** Full-screen project page. Rendered in place of the workspace panes while
 *  `projectScreenRepoPath` is set (mirrors SettingsScreen). Two tabs under a
 *  shared header: the Roadmap (what gets built) and Settings — the activity
 *  pulse plus the per-project config every agent in the project inherits.
 *  Keyed by the project's primary repo path; resolves the project_id and
 *  detected run config on open. */
export function ProjectScreen({ repoPath }: { repoPath: string }) {
  const close = useAppStore((s) => s.closeProjectScreen);
  const projects = useAppStore((s) => s.workspace?.projects);
  const [tab, setTab] = useState<ProjectTab>("roadmap");
  const [loaded, setLoaded] = useState<Loaded | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Lives here, not in <Roadmap>, because the header shows the same counts —
  // and they move when the user accepts a proposal.
  const roadmap = useRoadmap();

  // Custom display name for this repo, falling back to the folder basename.
  const name = projects?.find((p) => p.path === repoPath)?.name ?? basename(repoPath);
  // The project's repos (known once the project_id resolves). A single-repo
  // project reads as "the repo at this path"; multi-repo has no single
  // location, so the header shows the repo count instead.
  const projectRepoCount =
    loaded == null ? 1 : (projects?.filter((p) => p.project_id === loaded.projectId).length ?? 1);

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
      <ProjectHeader
        name={name}
        subtitle={projectRepoCount > 1 ? `${projectRepoCount} repositories` : repoPath}
        roadmap={roadmap}
        tab={tab}
        onTab={setTab}
        onClose={close}
      />

      {tab === "roadmap" ? (
        <Roadmap roadmap={roadmap} />
      ) : (
        <div className="ps-content">
          {error ? (
            <div className="ps-state text-sm">Couldn’t load project settings.</div>
          ) : !loaded ? (
            <div className="ps-state iflex-center text-sm">
              <Loader variant="inherit" /> Loading…
            </div>
          ) : (
            <div className="ps-sections">
              <ProjectPulse projectId={loaded.projectId} />
              <GeneralSection projectId={loaded.projectId} currentName={name} />
              <RunEnvSection
                projectId={loaded.projectId}
                rows={loaded.rows}
                ecosystem={loaded.ecosystem}
                initialOverrides={loaded.overrides}
              />
              <EnvVarsSection projectId={loaded.projectId} repoPath={repoPath} />
              <LinearSection projectId={loaded.projectId} />
              <VerifySection projectId={loaded.projectId} />
              <DeleteSection projectId={loaded.projectId} projectName={name} />
            </div>
          )}
        </div>
      )}
    </div>
  );
}
