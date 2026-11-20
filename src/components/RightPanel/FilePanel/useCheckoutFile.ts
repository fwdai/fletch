import { useEffect, useState } from "react";
import type { CheckoutFileContents, DiffBaseMode } from "@/api";
import { useEngine } from "@/api/engine";

/** Read one checkout file. `contents` is null until the read of this agent,
 *  path and base settles, and stays null when it fails (`error`). `dep` lets
 *  the caller force a re-read (e.g. when the file's revision moves), which
 *  keeps the current contents until the new ones land. */
export function useCheckoutFile(agentId: string, path: string, base?: DiffBaseMode, dep?: string) {
  const { api } = useEngine();
  const key = `${agentId}\n${base ?? ""}\n${path}`;
  const [state, setState] = useState<{ key: string; contents: CheckoutFileContents | null }>();

  // biome-ignore lint/correctness/useExhaustiveDependencies: `dep` is the caller's re-read trigger
  useEffect(() => {
    let cancelled = false;
    api
      .readCheckoutFile(agentId, path, base)
      .then((contents) => {
        if (!cancelled) setState({ key, contents });
      })
      .catch(() => {
        if (!cancelled) setState({ key, contents: null });
      });
    return () => {
      cancelled = true;
    };
  }, [agentId, path, base, dep, key, api]);

  const current = state?.key === key ? state : undefined;
  return {
    contents: current?.contents ?? null,
    error: current !== undefined && current.contents === null,
  };
}
