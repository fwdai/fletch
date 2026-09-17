import { useMemo } from "react";
import { EMPTY_AGENTS, useAppStore } from "@/store";
import { useRuns } from "@/workflows/run/useRuns";
import { buildReviewQueue, type ReviewItem } from "./queue";

/** The live fleet review queue: subscribes to the already-polled store slices +
 *  the reactive run list and folds them through the pure `buildReviewQueue`
 *  selector. Derives instantly from state we already hold, so there's no spinner
 *  — the queue paints with the pane. */
export function useReviewQueue(): ReviewItem[] {
  const agents = useAppStore((s) => s.workspace?.agents ?? EMPTY_AGENTS);
  const gitShortstats = useAppStore((s) => s.gitShortstats);
  const gitMeta = useAppStore((s) => s.gitMeta);
  const unseenResults = useAppStore((s) => s.unseenResults);
  const prStates = useAppStore((s) => s.prStates);
  const prChecks = useAppStore((s) => s.prChecks);
  const prComments = useAppStore((s) => s.prComments);
  const verificationReports = useAppStore((s) => s.verificationReports);
  const dismissed = useAppStore((s) => s.reviewDismissed);
  const runs = useRuns();

  return useMemo(
    () =>
      buildReviewQueue({
        agents,
        gitShortstats,
        gitMeta,
        unseenResults,
        prStates,
        prChecks,
        prComments,
        verificationReports,
        runs,
        dismissed,
      }),
    [
      agents,
      gitShortstats,
      gitMeta,
      unseenResults,
      prStates,
      prChecks,
      prComments,
      verificationReports,
      runs,
      dismissed,
    ],
  );
}
