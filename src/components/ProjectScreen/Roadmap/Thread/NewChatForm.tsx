import { useMemo, useState } from "react";
import { ModelPicker } from "@/components/Composer/ModelPicker";
import { Icon } from "@/components/Icon";
import { Button } from "@/components/ui/Button";
import { DEFAULT_PROVIDER_ID } from "@/data/providers";
import { useAppStore } from "@/store";
import type { ChatAgentPick } from "./usePmChats";

/** Choose who runs the next PM chat, and start it.
 *
 *  The Project Manager preset is the default, but any custom agent or bare
 *  provider is fair game — a chat is just a conversation with a repo attached,
 *  and someone who has tuned their own planning agent should be able to use it
 *  here. The picker is the composer's own `ModelPicker`, so the vocabulary
 *  (agent, then model) is the one the user already knows from spawning agents.
 *  Effort isn't offered: it belongs to the agent definition, not to one chat. */
export function NewChatForm({
  defaultAgentId,
  starting,
  onStart,
}: {
  /** The Project Manager preset, when it has resolved. */
  defaultAgentId: string | undefined;
  starting: boolean;
  onStart: (pick: ChatAgentPick) => void;
}) {
  const customAgents = useAppStore((s) => s.customAgents);
  const [picked, setPicked] = useState<ChatAgentPick | null>(null);

  // The default follows `defaultAgentId` until the user touches the picker —
  // the preset may still be seeding on first open, and the form must not lock
  // in a bare provider just because it rendered a frame early.
  const fallback = useMemo<ChatAgentPick>(() => {
    const pm = customAgents.find((a) => a.id === defaultAgentId);
    return pm
      ? { provider: pm.base, model: pm.model ?? undefined, customAgentId: pm.id }
      : { provider: DEFAULT_PROVIDER_ID };
  }, [customAgents, defaultAgentId]);
  const pick = picked ?? fallback;

  return (
    <div className="rm-newchat flex-center">
      <ModelPicker
        provider={pick.provider}
        model={pick.model}
        customAgentId={pick.customAgentId}
        onChange={(provider, model, customAgentId) => setPicked({ provider, model, customAgentId })}
      />
      <Button variant="primary" size="sm" disabled={starting} onClick={() => onStart(pick)}>
        <Icon name="sparkle" size={11} /> {starting ? "Starting…" : "Start chat"}
      </Button>
    </div>
  );
}
