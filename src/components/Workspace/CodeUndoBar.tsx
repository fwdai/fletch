import { useState } from "react";
import { Icon } from "@/components/Icon";
import { Button } from "@/components/ui/Button";
import { IconButton } from "@/components/ui/IconButton";
import { useAppStore } from "@/store";

/** Above the composer after a rewind restored an agent's code: says so and
 *  offers to put the code back as it was (the store's `codeUndo`, kept until
 *  the agent's next turn). */
export function CodeUndoBar({ agentId }: { agentId: string }) {
  const report = useAppStore((s) => s.codeUndo[agentId]);
  const undo = useAppStore((s) => s.undoCodeRestore);
  const dismiss = useAppStore((s) => s.dismissCodeUndo);
  const [busy, setBusy] = useState(false);
  if (!report) return null;

  return (
    <div className="code-undo text-sm" role="status">
      <Icon name="rewind" size={12} className="code-undo-icon" />
      <span>The code was restored to before that message.</span>
      <Button
        variant="link"
        size="sm"
        disabled={busy}
        onClick={async () => {
          setBusy(true);
          try {
            await undo(agentId);
          } finally {
            setBusy(false);
          }
        }}
      >
        Undo
      </Button>
      <IconButton size="xs" aria-label="Dismiss" onClick={() => dismiss(agentId)}>
        <Icon name="close" size={12} />
      </IconButton>
    </div>
  );
}
