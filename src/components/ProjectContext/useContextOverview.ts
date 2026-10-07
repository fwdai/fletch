import { useCallback, useEffect, useRef, useState } from "react";
import { api, type ContextOverview, onContextChanged } from "@/api";
import { useAppStore } from "@/store";

/** One project's context overview, read from the engine the UI is driving and
 *  re-read on every `context:changed` for that project — a write from this
 *  tab, another client or the host's own pipeline all land the same way.
 *
 *  `overview` is null until the first read lands; `error` is the last failed
 *  read's message. `reload` is for a caller that wants to be sure. */
export function useContextOverview(projectId: string): {
  overview: ContextOverview | null;
  error: string | null;
  reload: () => void;
} {
  const [overview, setOverview] = useState<ContextOverview | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Re-read (and re-subscribe) on a switch: both are bound to the transport
  // that was active when they were made.
  const environmentId = useAppStore((s) => s.activeEnvironmentId);
  // Every read takes the next number; only the newest one asked may answer.
  // That covers a slow answer for the previous project landing as this
  // one's, and two reads of the same project (the first load, then a
  // `context:changed`) resolving out of order. Bumped on unmount too, so
  // nothing in flight answers after the hook is gone.
  const latest = useRef(0);

  const reload = useCallback(() => {
    latest.current += 1;
    const mine = latest.current;
    api
      .contextOverview(projectId)
      .then((o) => {
        if (latest.current !== mine) return;
        setOverview(o);
        setError(null);
      })
      .catch((e) => {
        if (latest.current !== mine) return;
        setError(String(e));
      });
  }, [projectId]);

  // biome-ignore lint/correctness/useExhaustiveDependencies: environmentId is the intended re-run trigger, not an unused dep
  useEffect(() => {
    setOverview(null);
    setError(null);
    reload();
    let cancelled = false;
    let unlisten: (() => void) | null = null;
    onContextChanged((e) => {
      if (e.project_id === projectId) reload();
    }).then((un) => {
      if (cancelled) un();
      else unlisten = un;
    });
    return () => {
      cancelled = true;
      latest.current += 1;
      unlisten?.();
    };
  }, [projectId, environmentId, reload]);

  return { overview, error, reload };
}
