import { useMemo, useState } from "react";
import { ModelPicker } from "@/components/Composer/ModelPicker";
import { Icon } from "@/components/Icon";
import { Button } from "@/components/ui/Button";
import { DEFAULT_PROVIDER_ID } from "@/data/providers";
import { useAppStore } from "@/store";
import type { ChatAgentPick } from "./usePmChats";

/** Choose who runs the next PM chat, and start it.
 *
 *  The picker is the composer's own `ModelPicker`, so the vocabulary (agent,
 *  then model) is the one the user already knows from spawning agents. Effort
 *  isn't offered: it belongs to the agent definition, not to one chat.
 *
 *  Three things are narrowed for this surface.
 *
 *  The custom-agent list is limited to the Project Manager: the rest of the
 *  library is built to *write code*, and this chat is denied the publish ops
 *  backend-side, so offering a tester or an architect here is an offer the
 *  surface can't honour. Bare coding agents stay — talking to Claude or Codex
 *  about the roadmap is a real thing to want, which is why they are titled
 *  "Default agents" and sit *under* the PM rather than leading.
 *
 *  And the menu drops downward. This form renders inside the thread header's
 *  popover, near the top of the window; the composer's default upward menu
 *  opened straight off the top of the screen and was clipped. */
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
  /** Allow-list for the picker's custom-agent section: the PM preset alone. */
  const pmAgentIds = defaultAgentId ? [defaultAgentId] : [];

  return (
    <div className="rm-newchat flex-center">
      <ModelPicker
        provider={pick.provider}
        model={pick.model}
        customAgentId={pick.customAgentId}
        drop="down"
        // Empty while the preset seeds, which hides the section rather than
        // showing an empty one — the bare coding agents are still selectable.
        customAgentIds={pmAgentIds}
        sections={{ custom: "Project manager", providers: "Default agents", customFirst: true }}
        onChange={(provider, model, customAgentId) => setPicked({ provider, model, customAgentId })}
      />
      <Button variant="primary" size="sm" disabled={starting} onClick={() => onStart(pick)}>
        <Icon name="sparkle" size={11} /> {starting ? "Starting…" : "Start chat"}
      </Button>
    </div>
  );
}
