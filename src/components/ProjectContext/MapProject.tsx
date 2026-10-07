import { useState } from "react";
import { api, type ContextBootstrap } from "@/api";
import { Button } from "@/components/ui/Button";
import { useAppStore } from "@/store";

/** "Map project": records the modules the repository's layout describes
 *  (`context_bootstrap`, no model involved), then offers the mapping session —
 *  an ordinary workspace whose first message is the host's canned task —
 *  that writes the vision and what each module is for. */
export function MapProject({ projectId, disabled }: { projectId: string; disabled: boolean }) {
  const repoPath = useAppStore(
    (s) => s.workspace?.projects.find((p) => p.project_id === projectId)?.path,
  );
  const createDraft = useAppStore((s) => s.createDraft);
  const closeProjectScreen = useAppStore((s) => s.closeProjectScreen);
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<ContextBootstrap | null>(null);
  const [error, setError] = useState<string | null>(null);

  const map = () => {
    setBusy(true);
    setError(null);
    api
      .contextBootstrap(projectId)
      .then(setResult)
      .catch((e) => setError(String(e)))
      .finally(() => setBusy(false));
  };

  const startSession = async () => {
    if (!repoPath || !result) return;
    // The draft lives in the workspace, which this page covers; stay put if
    // it couldn't be created so the error is seen here.
    const draftId = await createDraft(repoPath, result.mapping_task);
    if (draftId) closeProjectScreen();
  };

  return (
    <>
      <Button variant="outline" size="sm" disabled={disabled || busy} onClick={map}>
        {busy ? "Mapping…" : "Map project"}
      </Button>
      {result && (
        <span className="pc-meta text-xs iflex-center">
          {result.created.length > 0
            ? `Recorded ${result.created.length} of ${result.modules} modules at ${result.commit.slice(0, 7)}.`
            : `All ${result.modules} modules are already recorded.`}{" "}
          <Button variant="ghost" size="sm" disabled={!repoPath} onClick={startSession}>
            Start mapping session
          </Button>
        </span>
      )}
      {error && <span className="pc-error text-sm">{error}</span>}
    </>
  );
}
