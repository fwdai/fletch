// run/useRunAgents.ts — a run's step agents for the sidebar expander. Lighter
// than useRunDetail (no journal, no messages): it fetches `wf_run_agents` only
// while `enabled` (the RunRow is expanded) and keeps the list live over the
// same `wf:run` / `wf:event` streams the monitor uses, so a step agent's status
// rail tracks reality without polling every collapsed run.

import { useEffect, useRef, useState } from "react";
import { type AgentRecord, api, onWfEvent, onWfRun } from "../../api";

export function useRunAgents(runId: string, enabled: boolean): AgentRecord[] {
  const [agents, setAgents] = useState<AgentRecord[]>([]);
  const pending = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    if (!enabled) {
      setAgents([]);
      return;
    }
    let cancelled = false;

    const refresh = async () => {
      try {
        const next = await api.wfRunAgents(runId);
        if (!cancelled) setAgents(next);
      } catch {
        /* transient — the next event or a re-expand retries */
      }
    };
    const schedule = () => {
      if (pending.current) return;
      pending.current = setTimeout(() => {
        pending.current = null;
        void refresh();
      }, 150);
    };

    void refresh();
    const offEvent = onWfEvent((e) => {
      if (e.run_id === runId) schedule();
    });
    const offRun = onWfRun((row) => {
      if (row.id === runId) schedule();
    });

    return () => {
      cancelled = true;
      if (pending.current) {
        clearTimeout(pending.current);
        // Disarm, not just cancel — a truthy ref would make the next mount's
        // schedule() no-op forever, freezing rows on their initial fetch.
        pending.current = null;
      }
      void offEvent.then((f) => f());
      void offRun.then((f) => f());
    };
  }, [runId, enabled]);

  return agents;
}
