import type { CheckRun, PrChecks } from "@desktop/api/types/pr";

/** One check's outcome, as the desktop's ChecksSection reads it: not done yet
 *  is pending, a completed run that is neither a success nor a skip (neutral,
 *  skipped, stale, none) failed. A skip counts as passed here — it is not
 *  something to fix and not something to wait for. */
export function checkOutcome(run: CheckRun): "passed" | "failed" | "pending" {
  if (run.status !== "completed") return "pending";
  switch (run.conclusion) {
    case "success":
    case "neutral":
    case "skipped":
    case "stale":
    case null:
      return "passed";
    default:
      return "failed"; // failure, timed_out, cancelled, action_required, …
  }
}

/** The rollup in a few words for the evidence row: progress while any run is
 *  still going, the red count once they are done, else all green. Null when
 *  the PR reports no checks at all. */
export function checksSummary(checks: PrChecks | null | undefined): string | null {
  if (!checks || checks.total === 0) return null;
  if (checks.pending > 0) return `${checks.passed + checks.failed} of ${checks.total} done`;
  if (checks.failed > 0) return `${checks.failed} failing`;
  return "all passing";
}
