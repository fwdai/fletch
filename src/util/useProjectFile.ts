import { useCallback, useEffect, useState } from "react";
import { api, onProjectFileChanged, type ProjectFileStatus } from "@/api";
import { useAppStore } from "@/store";

/** One project's `.fletch/settings.json` as the host reads it, kept live by
 *  `project_file:changed` — an edit to the file, or an approval from another
 *  client, moves it without a refetch by hand.
 *
 *  `status` is null until the first read lands, and stays null when the host
 *  cannot answer it. `approve` approves host patterns the files add and
 *  resolves once the host stored them. */
export function useProjectFile(projectId: string | null | undefined): {
  status: ProjectFileStatus | null;
  approve: (patterns: string[]) => Promise<void>;
} {
  const [status, setStatus] = useState<ProjectFileStatus | null>(null);
  // Re-read (and re-subscribe) on a switch: both are bound to the transport
  // that was active when they were made.
  const environmentId = useAppStore((s) => s.activeEnvironmentId);

  // biome-ignore lint/correctness/useExhaustiveDependencies: environmentId is the intended re-run trigger, not an unused dep
  useEffect(() => {
    setStatus(null);
    if (!projectId) return;
    let live = true;
    const read = () =>
      api
        .projectFileStatus(projectId)
        .then((s) => live && setStatus(s))
        .catch(() => {});
    read();
    const unlisten = onProjectFileChanged((e) => {
      if (e.project_id === projectId) read();
    });
    return () => {
      live = false;
      void unlisten.then((off) => off());
    };
  }, [projectId, environmentId]);

  const approve = useCallback(
    async (patterns: string[]) => {
      if (!projectId) return;
      setStatus(await api.projectFileApprove(projectId, patterns));
    },
    [projectId],
  );

  return { status, approve };
}
