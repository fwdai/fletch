// Whether an agent should read as "working" right now — the debounced view of
// `managedBusy` every chat surface renders from.
//
// The backend emits a transient `idle` between process spawn and the first
// turn's `running` (every process rests at Idle at spawn). Raw `busy` thus dips
// false→true mid-startup, which would flash the working strip and restart its
// timer. Hold "working" through brief dips: rise immediately, fall only after a
// short grace period.
//
// Extracted from ChatView so the Roadmap tab's PM chat behaves identically —
// two copies of this timing would drift, and the drift would be visible.

import { useEffect, useState } from "react";
import { useAppStore } from "@/store";

/** `awaitingInput` (from the transcript) means the last row is an unanswered
 *  question widget: the agent is waiting on the user, not working, so the
 *  spinner must be suppressed even though the turn is technically open. */
export function useLiveBusy(agentId: string, awaitingInput: boolean): boolean {
  const busy = useAppStore((s) => s.managedBusy[agentId] ?? false);
  const raw = busy && !awaitingInput;
  const [liveBusy, setLiveBusy] = useState(raw);

  useEffect(() => {
    if (raw) {
      setLiveBusy(true);
      return;
    }
    const t = window.setTimeout(() => setLiveBusy(false), 700);
    return () => window.clearTimeout(t);
  }, [raw]);

  return liveBusy;
}
