// RunView/useRunDetail.ts — the monitor's data source. Loads the run detail
// (run row + attempts + messages) and the full journal, then keeps both live:
// `wf:run` patches the row instantly (status / paused_reason), and `wf:event`
// triggers a coalesced refetch of the run detail plus the new journal page.
//
// The journal is paged forward from seq 0 (spec §7.2). Live `wf:event` envelopes
// carry no payload, so an append refetches the page after our max seq to pick up
// the payloads the timeline summarizes.

import { useEffect, useRef, useState } from "react";
import {
  type AgentRecord,
  api,
  onWfEvent,
  onWfRun,
  type WfEvent,
  type WfRunDetail,
} from "../../../api";

const PAGE = 500;

export interface RunDetailState {
  detail: WfRunDetail | null;
  events: WfEvent[];
  /** The run's step agents (live + archived), for rendering attempt chats. */
  agents: AgentRecord[];
  /** True until the first load resolves (distinguishes "loading" from "empty"). */
  loading: boolean;
}

/** Page the whole journal forward from `after`, following short pages to the end. */
async function loadAllEvents(runId: string, after: number): Promise<WfEvent[]> {
  const acc: WfEvent[] = [];
  let cursor = after;
  for (;;) {
    const page = await api.wfEvents(runId, cursor, PAGE);
    if (page.length === 0) break;
    acc.push(...page);
    cursor = page[page.length - 1].seq;
    if (page.length < PAGE) break;
  }
  return acc;
}

export function useRunDetail(runId: string): RunDetailState {
  const [detail, setDetail] = useState<WfRunDetail | null>(null);
  const [events, setEvents] = useState<WfEvent[]>([]);
  const [agents, setAgents] = useState<AgentRecord[]>([]);
  const [loading, setLoading] = useState(true);

  // Highest seq we hold, so live appends fetch only the tail.
  const maxSeq = useRef(0);

  useEffect(() => {
    // `cancelled` flips on unmount or a runId switch. Every fetch checks it
    // before applying its setters, so a response for the previous run can never
    // paint over the newly selected one (nor over an unmounted component).
    let cancelled = false;
    let pending: ReturnType<typeof setTimeout> | null = null;
    maxSeq.current = 0;
    setLoading(true);
    setEvents([]);
    setDetail(null);
    setAgents([]);

    const refresh = async () => {
      try {
        const [d, tail, runAgents] = await Promise.all([
          api.wfGetRun(runId),
          loadAllEvents(runId, maxSeq.current),
          api.wfRunAgents(runId),
        ]);
        if (cancelled) return;
        // `d` is null when the run no longer exists (deleted): clear the view so
        // the monitor falls to its "Run not found" state rather than showing a
        // stale run.
        setDetail(d);
        setAgents(runAgents);
        if (tail.length > 0) {
          maxSeq.current = Math.max(maxSeq.current, tail[tail.length - 1].seq);
          setEvents((prev) => {
            const seen = new Set(prev.map((e) => e.seq));
            const merged = prev.concat(tail.filter((e) => !seen.has(e.seq)));
            merged.sort((a, b) => a.seq - b.seq);
            return merged;
          });
        }
      } catch {
        /* transient — the next event or the caller's remount retries */
      }
    };

    void (async () => {
      await refresh();
      if (!cancelled) setLoading(false);
    })();

    const scheduleRefresh = () => {
      if (pending) return;
      pending = setTimeout(() => {
        pending = null;
        void refresh();
      }, 150);
    };

    const offEvent = onWfEvent((e) => {
      if (e.run_id === runId && e.seq > maxSeq.current) scheduleRefresh();
    });
    const offRun = onWfRun((row) => {
      if (row.id === runId) setDetail((prev) => (prev ? { ...prev, run: row } : prev));
    });

    return () => {
      cancelled = true;
      if (pending) clearTimeout(pending);
      void offEvent.then((f) => f());
      void offRun.then((f) => f());
    };
  }, [runId]);

  return { detail, events, agents, loading };
}
