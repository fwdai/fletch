import { useEffect, useState } from "react";
import { api } from "@/api";
import { loadRunOverrides, type SetupRow, toSetupRows } from "@/components/RunConfig";
import { useAppStore } from "@/store";

export interface RunConfig {
  projectId: string;
  rows: SetupRow[];
  ecosystem: string | null;
  overrides: Record<string, string>;
}

/** Resolve the project_id and detected run config for `repoPath`, then load
 *  the persisted overrides. Both must be ready before the editor mounts so the
 *  draft baseline is correct. */
export function useRunConfig(repoPath: string) {
  const [loaded, setLoaded] = useState<RunConfig | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setLoaded(null);
    setError(null);
    (async () => {
      try {
        // Run detection is not on a host's table yet, so a remote project's
        // settings still open — with the project id the workspace already
        // carries and no detected rows — rather than failing whole: every pane
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

  return { loaded, error };
}
