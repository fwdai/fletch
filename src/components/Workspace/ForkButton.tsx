import { useState } from "react";
import { Icon } from "@/components/Icon";
import { IconButton } from "@/components/ui/IconButton";
import { useAppStore } from "@/store";

/** "Fork from here" affordance under an ended turn. Creates a new workspace
 *  seeded with a clean worktree (from the parent's base branch) and the
 *  conversation carried up to this turn, then opens it. The token-slicing /
 *  branch-a-direction entry point.
 *
 *  Code is `clean` this slice (carrying the parent's working tree is a follow-up
 *  and will turn this into a small menu); context is fixed to "up to here" since
 *  that's what forking *from a turn* means. */
export function ForkButton({ agentId, upToPrompt }: { agentId: string; upToPrompt: number }) {
  const forkAgent = useAppStore((s) => s.forkAgent);
  const [forking, setForking] = useState(false);

  const onFork = async () => {
    if (forking) return;
    setForking(true);
    try {
      await forkAgent(agentId, "clean", { kind: "up_to_message", prompt: upToPrompt });
    } finally {
      setForking(false);
    }
  };

  return (
    <IconButton
      size="xs"
      tip="Fork a new workspace from here (carries the conversation up to this point)"
      className="turn-fork"
      onClick={onFork}
      disabled={forking}
      aria-label="Fork a new workspace from here"
    >
      <Icon name="branch" size={12} />
    </IconButton>
  );
}
