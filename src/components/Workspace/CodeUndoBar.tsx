import { useEffect, useState } from "react";
import { Icon } from "@/components/Icon";
import { Button } from "@/components/ui/Button";
import { IconButton } from "@/components/ui/IconButton";
import { useAppStore } from "@/store";
import { useGate } from "@/store/capabilities";

/** Above the composer while a rewind's code restore can still be undone: says
 *  so, and offers to put the code back as it was, or to keep it (which lets
 *  the backend's undo point go). Asks the backend as the chat opens, so the
 *  offer survives a restart. */
export function CodeUndoBar({ agentId }: { agentId: string }) {
  const rewindGate = useGate("rewind");
  const undoable = useAppStore((s) => agentId in s.codeUndo);
  const refresh = useAppStore((s) => s.refreshCodeUndo);
  const undo = useAppStore((s) => s.undoCodeRestore);
  const discard = useAppStore((s) => s.discardCodeUndo);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (!rewindGate) void refresh(agentId);
  }, [agentId, rewindGate, refresh]);

  if (rewindGate || !undoable) return null;

  const run = async (action: (agentId: string) => Promise<void>) => {
    setBusy(true);
    try {
      await action(agentId);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="code-undo text-sm" role="status">
      <Icon name="rewind" size={12} className="code-undo-icon" />
      <span>The code was restored to before that message.</span>
      <Button variant="link" size="sm" disabled={busy} onClick={() => run(undo)}>
        Undo
      </Button>
      <IconButton
        size="xs"
        aria-label="Keep the restored code"
        tip="Keep the restored code"
        disabled={busy}
        onClick={() => run(discard)}
      >
        <Icon name="close" size={12} />
      </IconButton>
    </div>
  );
}
