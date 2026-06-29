// run/useWorkflows.ts — the saved workflow definitions, loaded once (the picker
// reads these; they change rarely, so no live subscription).

import { useEffect, useState } from "react";
import { listWorkflows, type Workflow } from "../storage";

export function useWorkflows(): Workflow[] {
  const [workflows, setWorkflows] = useState<Workflow[]>([]);
  useEffect(() => {
    listWorkflows()
      .then(setWorkflows)
      .catch(() => {});
  }, []);
  return workflows;
}
