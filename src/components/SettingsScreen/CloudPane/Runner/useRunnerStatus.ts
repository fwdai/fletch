import { useEffect, useState } from "react";
import type { CloudRunnerStatus } from "@/api";
import { useAppStore } from "@/store";

/** How often the runner's status is re-read while it is changing on its own. */
export const RUNNER_POLL_MS = 3_000;

/** The runner states that change on their own. */
const TRANSITIONAL: ReadonlySet<CloudRunnerStatus["state"]> = new Set([
  "setting_up",
  "starting",
  "stopping",
]);

/** Read the runner's status when the pane opens, and again every few seconds
 *  only while it is setting up, starting or stopping: a read asks the
 *  provider, so nothing polls a runner that is standing still. Answers why the
 *  first read failed, if it did. */
export function useRunnerStatus(): string | null {
  const refresh = useAppStore((s) => s.refreshCloudRunner);
  const state = useAppStore((s) => s.cloudRunner?.state);
  const [error, setError] = useState<string | null>(null);
  const transitional = !!state && TRANSITIONAL.has(state);

  useEffect(() => {
    refresh().then(
      () => setError(null),
      (e) => setError(String(e)),
    );
  }, [refresh]);

  useEffect(() => {
    if (!transitional) return;
    const timer = window.setInterval(() => void refresh().catch(() => {}), RUNNER_POLL_MS);
    return () => window.clearInterval(timer);
  }, [transitional, refresh]);

  return error;
}
