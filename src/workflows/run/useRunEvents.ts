// run/useRunEvents.ts — the journal, live. Pages the run's whole event log
// forward from seq 0 (spec §7.2) and follows `wf:event` appends with a
// coalesced tail fetch (the envelope carries no payload). Shared by the
// Activity panel (its timeline) and useRunDetail (the run monitor needs the
// same journal for its paused/evidence lookups).

import { useEffect, useRef, useState } from "react";
import { api, onWfEvent, type WfEvent } from "../../api";

const PAGE = 500;

/** Page the whole journal forward from `after`, following short pages to the end. */
export async function loadAllEvents(runId: string, after: number): Promise<WfEvent[]> {
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

export function useRunEvents(runId: string): { events: WfEvent[]; loading: boolean } {
  const [events, setEvents] = useState<WfEvent[]>([]);
  const [loading, setLoading] = useState(true);
  const maxSeq = useRef(0);

  useEffect(() => {
    let cancelled = false;
    let pending: ReturnType<typeof setTimeout> | null = null;
    maxSeq.current = 0;
    setLoading(true);
    setEvents([]);

    const refresh = async () => {
      try {
        const tail = await loadAllEvents(runId, maxSeq.current);
        if (cancelled || tail.length === 0) return;
        maxSeq.current = Math.max(maxSeq.current, tail[tail.length - 1].seq);
        setEvents((prev) => {
          const seen = new Set(prev.map((e) => e.seq));
          const merged = prev.concat(tail.filter((e) => !seen.has(e.seq)));
          merged.sort((a, b) => a.seq - b.seq);
          return merged;
        });
      } catch {
        /* transient — the next event retries */
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

    return () => {
      cancelled = true;
      if (pending) clearTimeout(pending);
      void offEvent.then((f) => f());
    };
  }, [runId]);

  return { events, loading };
}
