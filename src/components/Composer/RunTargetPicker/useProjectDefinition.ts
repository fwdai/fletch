import { useCallback, useState } from "react";
import type { ProjectDefinition } from "@/api";
import { useEngine } from "@/api/engine";

/** The project as the host the caller is scoped to describes it
 *  (`project_definition`), read on `check`: whether it can run on another
 *  device turns on its repositories' remotes, which change as they are
 *  published. Null until a read for this project lands; a failed read keeps
 *  what was there. Knowing nothing blocks nothing, and the spawn reads it
 *  again anyway. */
export function useProjectDefinition(projectId: string): {
  definition: ProjectDefinition | null;
  check: () => void;
} {
  const { api } = useEngine();
  const [read, setRead] = useState<ProjectDefinition | null>(null);
  const check = useCallback(() => {
    if (!projectId) return;
    api.projectDefinition(projectId).then(setRead, () => {});
  }, [api, projectId]);
  return { definition: read?.project_id === projectId ? read : null, check };
}
