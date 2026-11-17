import { useCallback, useEffect, useRef, useState } from "react";
import type { ContextOverview } from "@/api";
import { useEngine } from "@/api/engine";

/** One project's context overview, read from the host the caller is scoped to
 *  and re-read on every `context:changed` for that project — a write from
 *  this tab, another client or the host's own pipeline all land the same way.
 *
 *  `overview` is null until the first read lands; `error` is the last failed
 *  read's message. `reload` is for a caller that wants to be sure. */
export function useContextOverview(projectId: string): {
  overview: ContextOverview | null;
  error: string | null;
  reload: () => void;
} {
  const { api, events } = useEngine();
  const [overview, setOverview] = useState<ContextOverview | null>(null);
  const [error, setError] = useState<string | null>(null);
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
  }, [projectId, api]);

  // `reload` and `events` are the scope's host's: another host is a re-read.
  useEffect(() => {
    setOverview(null);
    setError(null);
    reload();
    let cancelled = false;
    let unlisten: (() => void) | null = null;
    events
      .onContextChanged((e) => {
        if (e.project_id === projectId) reload();
      })
      .then((un) => {
        if (cancelled) un();
        else unlisten = un;
      });
    return () => {
      cancelled = true;
      latest.current += 1;
      unlisten?.();
    };
  }, [projectId, reload, events]);

  return { overview, error, reload };
}
