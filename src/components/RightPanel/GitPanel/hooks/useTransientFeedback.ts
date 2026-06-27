import { useCallback, useEffect, useRef, useState } from "react";

/** Two short-lived UI signals the panel shows for actions that otherwise
 *  leave no visible trace:
 *  - `busy`: the in-flight async verb (dimmed body, spinner, busy CTA),
 *  - `notice`: a transient confirmation toast (push/pull/rebase, delegation).
 *  Both reset on agent switch so they don't leak between worktrees. */
export function useTransientFeedback(agentId: string) {
  // In-flight async action — holds the present-tense verb to show.
  const [busy, setBusy] = useState<string | null>(null);
  const runBusy = useCallback(async (label: string, fn: () => Promise<unknown>) => {
    setBusy(label);
    try {
      return await fn();
    } finally {
      setBusy(null);
    }
  }, []);

  // Transient confirmation for fire-and-forget actions.
  const [notice, setNotice] = useState<string | null>(null);
  const noticeTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const showNotice = useCallback((m: string) => {
    setNotice(m);
    if (noticeTimer.current) clearTimeout(noticeTimer.current);
    noticeTimer.current = setTimeout(() => setNotice(null), 3500);
  }, []);
  useEffect(
    () => () => {
      if (noticeTimer.current) clearTimeout(noticeTimer.current);
    },
    [],
  );

  useEffect(() => {
    setNotice(null);
    setBusy(null);
  }, [agentId]);

  return { busy, runBusy, notice, showNotice };
}
