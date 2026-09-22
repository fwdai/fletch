import type { SpawnStage } from "@/api";

/** What each stage of a spawn reads as in the UI. Shared by the composer
 *  placeholder and the sidebar spinner's tooltip so the two can't disagree
 *  about what the agent is doing. */
const LABELS: Record<SpawnStage, string> = {
  preparing: "Preparing workspace…",
  cloning: "Cloning repository…",
  indexing: "Warming code index…",
  carrying: "Carrying over your changes…",
  attaching_repos: "Checking out the project's other repos…",
  starting: "Starting agent…",
};

/** The label for a spawn's current stage, or `undefined` when there is none to
 *  show — no stage yet (nothing is snapshotted, so a client that joins
 *  mid-spawn has none until the next one fires), or a stage from a host newer
 *  than this client. Callers fall back to their generic wording. */
export function spawnStageLabel(
  progress: { stage: SpawnStage; detail: string | null } | undefined,
): string | undefined {
  if (!progress) return undefined;
  const label = LABELS[progress.stage];
  if (!label) return undefined;
  return progress.detail ? `${label} (${progress.detail})` : label;
}
