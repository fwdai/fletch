// Which workflow a roadmap item runs under.
//
// Two facts, loaded once per project: the workflow definitions on this machine,
// and the project's default. The board needs both to *say* what a queued item
// will run under before it runs, and the item form needs both to let the user
// override it.
//
// The default is read through `loadPipelinePrefs` — the same `project_settings`
// key the composer writes (`src/workflows/run/projectPipeline.ts`) and the same
// key the Rust drainer falls back to. One "the workflow this project runs",
// three readers.

import { useCallback, useEffect, useMemo, useState } from "react";
import { api } from "@/api";
import { loadPipelinePrefs } from "@/workflows/run/projectPipeline";
import type { Definition } from "@/workflows/spec";

export interface ProjectWorkflows {
  definitions: Definition[];
  /** The project's default definition id, or null if none is set. */
  defaultId: string | null;
  /** The default's name, or null when unset or pointing at a deleted one. */
  defaultName: string | null;
  /** True once both loads have settled — until then "no default" is unknown,
   *  not false, and the UI must not warn about it. */
  ready: boolean;
  /** What an item will actually run under: its override, else the project
   *  default. `null` means nothing would run it — the queue would stall. */
  resolve: (workflowDefId: string | null) => Definition | null;
}

export function useProjectWorkflows(projectId: string | null): ProjectWorkflows {
  const [definitions, setDefinitions] = useState<Definition[]>([]);
  const [defaultId, setDefaultId] = useState<string | null>(null);
  const [ready, setReady] = useState(false);

  useEffect(() => {
    if (!projectId) {
      setDefinitions([]);
      setDefaultId(null);
      setReady(false);
      return;
    }
    let alive = true;
    setReady(false);
    Promise.all([api.wfDefList().catch(() => []), loadPipelinePrefs(projectId)])
      .then(([defs, prefs]) => {
        if (!alive) return;
        setDefinitions(defs);
        setDefaultId(prefs.defaultWorkflowId);
      })
      .finally(() => {
        if (alive) setReady(true);
      });
    return () => {
      alive = false;
    };
  }, [projectId]);

  const resolve = useCallback(
    (workflowDefId: string | null) => {
      const id = workflowDefId ?? defaultId;
      return definitions.find((d) => d.id === id) ?? null;
    },
    [definitions, defaultId],
  );

  const defaultName = useMemo(
    () => definitions.find((d) => d.id === defaultId)?.name ?? null,
    [definitions, defaultId],
  );

  return { definitions, defaultId, defaultName, ready, resolve };
}
