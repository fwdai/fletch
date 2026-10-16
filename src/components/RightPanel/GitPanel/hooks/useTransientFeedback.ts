import { useCallback } from "react";
import { useAppStore } from "@/store";

/** Two short-lived UI signals the panel shows for actions that otherwise
 *  leave no visible trace:
 *  - `busy`: the in-flight async verb (dimmed body, spinner, busy CTA),
 *  - `notice`: a transient confirmation toast (push/pull/rebase, delegation).
 *  Both live in the store, keyed by checkout: an action outlives the section
 *  that started it (the section mounts per checkout), so the lock and its
 *  outcome have to as well, or coming back to the checkout mid-push would
 *  offer the push again. */
export function useTransientFeedback(checkout: string) {
  const busy = useAppStore((s) => s.gitBusy[checkout] ?? null);
  const notice = useAppStore((s) => s.gitNotices[checkout] ?? null);
  const runGitBusy = useAppStore((s) => s.runGitBusy);
  const noteGitAction = useAppStore((s) => s.noteGitAction);
  const runBusy = useCallback(
    (label: string, fn: () => Promise<unknown>) => runGitBusy(checkout, label, fn),
    [checkout, runGitBusy],
  );
  const showNotice = useCallback(
    (text: string) => noteGitAction(checkout, text),
    [checkout, noteGitAction],
  );
  return { busy, runBusy, notice, showNotice };
}
