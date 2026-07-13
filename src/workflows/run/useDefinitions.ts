// run/useDefinitions.ts — the stored workflow definitions (spec §13, `wf_def_*`),
// newest-edited first. Used by the launch composer and the empty-state toggle.
// Read-only and load-once, mirroring the old v0 hook it replaces; definitions are
// edited in Settings, not from these surfaces.

import { useEffect, useState } from "react";
import { api } from "../../api";
import type { Definition } from "../spec";

export function useDefinitions(): { definitions: Definition[]; loading: boolean } {
  const [definitions, setDefinitions] = useState<Definition[]>([]);
  const [loading, setLoading] = useState(true);
  useEffect(() => {
    let alive = true;
    api
      .wfDefList()
      .then((d) => {
        if (alive) setDefinitions(d);
      })
      .catch(() => {})
      .finally(() => {
        if (alive) setLoading(false);
      });
    return () => {
      alive = false;
    };
  }, []);
  return { definitions, loading };
}
