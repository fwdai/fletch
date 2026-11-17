import { useCallback, useEffect, useState } from "react";
import type { ProjectFileStatus } from "@/api";
import { useEngine } from "@/api/engine";

/** One project's `.fletch/settings.json` as the host the caller is scoped to
 *  reads it, kept live by `project_file:changed` — an edit to the file, or an
 *  approval from another client, moves it without a refetch by hand.
 *
 *  `status` is null until the first read lands, and stays null when the host
 *  cannot answer it. `approve` approves host patterns the files add and
 *  resolves once the host stored them. */
export function useProjectFile(projectId: string | null | undefined): {
  status: ProjectFileStatus | null;
  approve: (patterns: string[]) => Promise<void>;
} {
  const { api, events } = useEngine();
  const [status, setStatus] = useState<ProjectFileStatus | null>(null);

  // `api` and `events` are the scope's host's: another host is a re-read.
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
    const unlisten = events.onProjectFileChanged((e) => {
      if (e.project_id === projectId) read();
    });
    return () => {
      live = false;
      void unlisten.then((off) => off());
    };
  }, [projectId, api, events]);

  const approve = useCallback(
    async (patterns: string[]) => {
      if (!projectId) return;
      setStatus(await api.projectFileApprove(projectId, patterns));
    },
    [projectId, api],
  );

  return { status, approve };
}
