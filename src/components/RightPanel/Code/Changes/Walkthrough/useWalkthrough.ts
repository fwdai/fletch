// Where an agent's walkthrough comes from — its own tour when it wrote one with
// at least a step, else its transcript — and which step the reader is on.
import { useCallback, useEffect, useMemo, useState } from "react";
import { type AgentRecord, api, type FileStatus, onTourChanged, type Tour } from "@/api";
import { useTranscript } from "@/components/Workspace/messages/useTranscript";
import { deriveStepsFromTranscript, type Step, stepsFromTour } from "./model";

export type WalkthroughSource = "tour" | "transcript" | "none";

export interface WalkthroughState {
  steps: Step[];
  source: WalkthroughSource;
  /** The step the reader is on; 0 for an agent they haven't walked yet. */
  index: number;
  setIndex: (index: number) => void;
}

export function useWalkthrough(agent: AgentRecord, files: FileStatus[]): WalkthroughState {
  const tour = useTour(agent.id);
  const { items } = useTranscript(agent);

  const steps = useMemo(() => {
    // Nothing until the tour has loaded, so a transcript walkthrough doesn't
    // flash up and then give way to the tour.
    if (tour === undefined) return [];
    if (tour && tour.steps.length > 0) return stepsFromTour(tour, files);
    return deriveStepsFromTranscript(items, files);
  }, [tour, items, files]);

  const source: WalkthroughSource =
    steps.length === 0 ? "none" : tour && tour.steps.length > 0 ? "tour" : "transcript";

  // Tagged with the agent so switching agents starts over at the first step.
  const [position, setPosition] = useState({ agentId: agent.id, index: 0 });
  const index =
    position.agentId === agent.id ? Math.min(position.index, Math.max(steps.length - 1, 0)) : 0;
  const setIndex = useCallback(
    (next: number) => setPosition({ agentId: agent.id, index: next }),
    [agent.id],
  );

  return { steps, source, index, setIndex };
}

/** The agent's tour, reloaded each time it writes one. Undefined until the
 *  first load settles; null when it has none — or when the host can't say (a
 *  rejected `getTour`, e.g. a host without the command), which reads the same. */
function useTour(agentId: string): Tour | null | undefined {
  const [loaded, setLoaded] = useState<{ agentId: string; tour: Tour | null } | null>(null);

  useEffect(() => {
    let cancelled = false;
    let latest = 0;
    const load = () => {
      latest += 1;
      const call = latest;
      void api
        .getTour(agentId)
        .catch(() => null)
        .then((tour) => {
          // Only the newest load lands: an older one can settle after it.
          if (!cancelled && call === latest) setLoaded({ agentId, tour });
        });
    };
    load();
    const off = onTourChanged((e) => {
      if (e.agent_id === agentId) load();
    });
    return () => {
      cancelled = true;
      void off.then((f) => f());
    };
  }, [agentId]);

  return loaded?.agentId === agentId ? loaded.tour : undefined;
}
