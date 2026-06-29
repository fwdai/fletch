// run/useRuns.ts — reactive list of workflow runs (engine notifications + slow
// poll fallback). Used by the sidebar entity provider.

import { useCallback, useEffect, useState } from "react";
import { resumeActiveRuns, subscribeRuns } from "./engine";
import { listRuns } from "./storage";
import type { WorkflowRun } from "./types";

// Resume interrupted runs once per app session. The sidebar (hence useRuns)
// mounts at startup, so this is our app-start hook without a dedicated seam.
let resumeKicked = false;

export function useRuns(): WorkflowRun[] {
  const [runs, setRuns] = useState<WorkflowRun[]>([]);
  const reload = useCallback(async () => {
    try {
      setRuns(await listRuns());
    } catch {
      /* transient */
    }
  }, []);
  useEffect(() => {
    if (!resumeKicked) {
      resumeKicked = true;
      void resumeActiveRuns();
    }
    void reload();
    const off = subscribeRuns(() => void reload());
    const timer = setInterval(() => void reload(), 2000);
    return () => {
      off();
      clearInterval(timer);
    };
  }, [reload]);
  return runs;
}
