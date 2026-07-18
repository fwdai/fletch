import { useEffect, useState } from "react";
import { api } from "@/api";
import { Icon } from "@/components/Icon";
import { loadRunOverrides, type SetupRow, toSetupRows } from "@/components/RunConfig";
import { Loader } from "@/components/ui/Loader";
import { useAppStore } from "@/store";
import { basename } from "@/util/format";
import { DeleteSection } from "./DeleteSection";
import { EnvVarsSection } from "./EnvVarsSection";
import { GeneralSection } from "./GeneralSection";
import { ProjectPulse } from "./ProjectPulse";
import { RunEnvSection } from "./RunEnvSection";
import { VerifySection } from "./VerifySection";

interface Loaded {
  projectId: string;
  rows: SetupRow[];
  ecosystem: string | null;
  overrides: Record<string, string>;
}

/** Project Settings modal. A centered overlay (mirrors the History sheet) for
 *  editing per-project defaults — chiefly the run configuration every agent in
 *  the project inherits. Sections stack in one scrollable page. Keyed by the
 *  sidebar's repo path; resolves the project_id and detected run config on open. */
export function ProjectSettings({ repoPath }: { repoPath: string }) {
  const close = useAppStore((s) => s.closeProjectSettings);
  const projects = useAppStore((s) => s.workspace?.projects);
  const [loaded, setLoaded] = useState<Loaded | null>(null);
  const [error, setError] = useState<string | null>(null);

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

  // Close on Escape.
  useEffect(() => {
    const h = (e: KeyboardEvent) => {
      if (e.key === "Escape") close();
    };
    document.addEventListener("keydown", h);
    return () => document.removeEventListener("keydown", h);
  }, [close]);

  return (
    <div className="ps-overlay" onClick={close}>
      <div
        className="ps-modal"
        role="dialog"
        aria-modal="true"
        aria-label="Project settings"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="ps-head">
          <div className="ps-id">
            <div className="ps-title text-lg truncate">{name}</div>
            <div className="ps-path mono text-xs truncate">
              {projectRepoCount > 1 ? `${projectRepoCount} repositories` : repoPath}
            </div>
          </div>
          <button className="ps-x iflex-center" onClick={close} aria-label="Close">
            <Icon name="close" size={13} />
          </button>
        </div>

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
              <VerifySection projectId={loaded.projectId} />
              <DeleteSection projectId={loaded.projectId} projectName={name} />
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
