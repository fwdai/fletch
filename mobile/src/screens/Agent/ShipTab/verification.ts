import type { VerificationReport } from "@desktop/api/types/verify";

export type TestsEvidence = "passed" | "failed";

/** The definitive tests verdict from a turn-end verification, or `undefined`
 *  when there is nothing to show (no report, or its `test` check never ran).
 *  Same rule as the desktop's Mission Control card (`queue.ts`): a failing,
 *  timed-out or setup-failed test all read as "failed"; a `skipped` test (no
 *  command) is not a verdict. */
export function testsEvidence(report: VerificationReport | undefined): TestsEvidence | undefined {
  const test = report?.checks.find((c) => c.name === "test");
  if (!test || test.outcome === "skipped") return undefined;
  return test.outcome === "passed" ? "passed" : "failed";
}

export const testsText = (e: TestsEvidence) =>
  e === "passed" ? "Tests passed at turn end" : "Tests failed at turn end";
