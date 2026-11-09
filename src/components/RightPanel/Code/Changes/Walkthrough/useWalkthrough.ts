// Where an agent's walkthrough comes from — its own tour when it wrote one with
// at least a step, else its transcript — and which step the reader is on.
import { useCallback, useEffect, useMemo, useState } from "react";
import { type AgentRecord, api, type FileStatus, onTourChanged, type Tour } from "@/api";
import type { ViewItem } from "@/components/Workspace/messages/pair";
import { useHistoryLoad, useTranscriptFrom } from "@/components/Workspace/messages/useTranscript";
import { useAppStore } from "@/store";
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
  const loaded = useTour(agent.id);
  // A tour with at least one step; null for an agent that wrote none.
  const tour = loaded != null && loaded.steps.length > 0 ? loaded : null;
  // The transcript is read only once the tour has loaded and is absent: a
  // transcript walkthrough must not flash up and give way to the tour, and
  // with a tour nothing here should re-run on every streamed token.
  const items = useTranscriptItems(agent, loaded !== undefined && tour === null);

  const steps = useMemo(
    () => (tour ? stepsFromTour(tour, files) : deriveStepsFromTranscript(items, files)),
    [tour, items, files],
  );

  const source: WalkthroughSource = steps.length === 0 ? "none" : tour ? "tour" : "transcript";

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

/** The agent's transcript as the chat shows it, while `enabled`; an empty
 *  transcript otherwise — and no subscription to the log, so a streaming turn
 *  doesn't re-render the caller. */
function useTranscriptItems(agent: AgentRecord, enabled: boolean): ViewItem[] {
  useHistoryLoad(agent);
  const log = useAppStore((s) => (enabled ? s.managedLogs[agent.id] : undefined));
  return useTranscriptFrom(agent, log, false).items;
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
