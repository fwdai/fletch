import { useEffect, useState } from "react";
import { api } from "@/api";
import { ProjectContext } from "@/components/ProjectContext";
import { loadRunOverrides, type SetupRow, toSetupRows } from "@/components/RunConfig";
import { Loader } from "@/components/ui/Loader";
import { useAppStore } from "@/store";
import { basename } from "@/util/format";
import { Activity } from "./Activity";
import { ProjectHeader } from "./ProjectHeader";
import { Roadmap, useRoadmap } from "./Roadmap";
import { ProjectSettings } from "./Settings";

interface Loaded {
  projectId: string;
  rows: SetupRow[];
  ecosystem: string | null;
  overrides: Record<string, string>;
}

/** Full-screen project page. Rendered in place of the workspace panes while
 *  `projectScreenRepoPath` is set (mirrors SettingsScreen). Four tabs under a
 *  header that names the project: the Roadmap (what gets built next), Activity
 *  (what has been built), Context (what the host knows about the project, while
 *  the layer's developer gate is open) and Settings (the per-project config
 *  every agent in the project inherits).
 *
 *  Keyed by the project's primary repo path; resolves the project_id and
 *  detected run config on open. Both non-roadmap tabs need the resolved
 *  project_id, so they share one load gate. */
export function ProjectScreen({ repoPath }: { repoPath: string }) {
  const close = useAppStore((s) => s.closeProjectScreen);
  const projects = useAppStore((s) => s.workspace?.projects);
  // The tab lives in the store so whoever opened the screen picks it — ⌘⇧,
  // lands on Settings, the sidebar's "Open project" on the tab last left.
  const tab = useAppStore((s) => s.projectScreenTab);
  const contextLayerEnabled = useAppStore((s) => s.contextLayerEnabled);
  const setTab = useAppStore((s) => s.setProjectScreenTab);
  const [loaded, setLoaded] = useState<Loaded | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Lives here, not in <Roadmap>, because the header shows the same counts —
  // and they move when the user accepts a proposal. Keyed by the repo, which
  // the hook resolves to the owning project (one roadmap per project).
  const roadmap = useRoadmap(repoPath);

  // Custom display name for this repo, falling back to the folder basename.
  const project = projects?.find((p) => p.path === repoPath);
  const name = project?.name ?? basename(repoPath);
  // A Context tab whose developer gate closed falls back to Settings.
  const onSettings = tab === "settings" || (tab === "context" && !contextLayerEnabled);

  // Resolve project_id + detected run config for the repo, then load the
  // persisted overrides. Both must be ready before the editor mounts so the
  // draft baseline is correct.
  useEffect(() => {
    let cancelled = false;
    setLoaded(null);
    setError(null);
    (async () => {
      try {
        // Run detection is not on a host's table yet, so a remote project page
        // still opens — with the project id the workspace already carries and
        // no detected rows — rather than failing whole: every section below
        // but detection edits the host through ops it does answer.
        const known = useAppStore
          .getState()
          .workspace?.projects.find((p) => p.path === repoPath)?.project_id;
        const { project_id, configs } = await api.projectRunConfig(repoPath).catch((err) => {
          if (!known) throw err;
          return { project_id: known, configs: [] };
        });
        const overrides = await loadRunOverrides(project_id).catch((err) => {
          console.error("loadRunOverrides failed", err);
          return {};
        });
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
        project={project ?? { name }}
        roadmap={roadmap}
        tab={tab}
        onTab={setTab}
        onClose={close}
      />

      {tab === "roadmap" ? (
        <Roadmap roadmap={roadmap} repoPath={repoPath} />
      ) : loaded && onSettings ? (
        <ProjectSettings {...loaded} repoPath={repoPath} name={name} />
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
              <ProjectContext projectId={loaded.projectId} />
            )}
          </div>
        </div>
      )}
    </div>
  );
}
