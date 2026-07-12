// run/useDefinitions.ts — the stored workflow definitions (spec §13, `wf_def_*`),
// newest-edited first. Used by the launch composer and the empty-state toggle.
// Read-only and load-once, mirroring the old v0 hook it replaces; definitions are
// edited in Settings, not from these surfaces.

import { useEffect, useState } from "react";
import { api } from "../../api";
import type { Definition } from "../spec";

export function useDefinitions(): Definition[] {
  const [defs, setDefs] = useState<Definition[]>([]);
  useEffect(() => {
    api
      .wfDefList()
      .then(setDefs)
      .catch(() => {});
  }, []);
  return defs;
}
