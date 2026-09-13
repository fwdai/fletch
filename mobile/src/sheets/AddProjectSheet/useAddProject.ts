import { useCallback, useEffect, useState } from "react";
import { useStore } from "../../store";

export type AddProjectTab = "folder" | "clone";

/** Runs one add-project op. The store closes the sheet on success, so nothing
 *  here has to know which op it was. */
export type RunAddProject = (op: () => Promise<unknown>) => Promise<void>;

/** State the two forms share: which one is showing, whether an op is in
 *  flight, and the error text the last one failed with. */
export function useAddProject(open: boolean, initialTab: AddProjectTab = "folder") {
  const [tab, setTab] = useState<AddProjectTab>(initialTab);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const clearError = useStore((s) => s.clearError);

  // A fresh sheet every time it opens: a failed attempt must not greet the
  // next one. The store's own `lastError` is cleared with it, since `guard`
  // recorded this error there as well.
  useEffect(() => {
    if (!open) return;
    setTab(initialTab);
    setBusy(false);
    setError(null);
    clearError();
  }, [open, initialTab, clearError]);

  const switchTab = useCallback((next: string) => {
    setTab(next as AddProjectTab);
    setError(null);
  }, []);

  const run = useCallback<RunAddProject>(async (op) => {
    setBusy(true);
    setError(null);
    try {
      await op();
    } catch (e) {
      // The sheet stays open on the text the host sent, so the user can fix
      // the destination or the spec and try again.
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }, []);

  return { tab, switchTab, busy, error, setError, run };
}
