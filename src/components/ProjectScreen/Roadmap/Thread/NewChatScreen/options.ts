import { useAgentAvailability } from "@/components/Composer/availability";
import { PROVIDERS, type Provider, providerLabel } from "@/data/providers";
import type { CustomAgent } from "@/storage/customAgents";
import { useAppStore } from "@/store";
import type { ChatAgentPick } from "../usePmChats";

/** One choosable agent on the new-chat screen. Exactly one of `custom` /
 *  `provider` is set — that's what decides how the card draws its mark and
 *  whether it offers a model. */
export interface AgentOption {
  /** Stable identity for selection and keyboard shortcuts: the custom agent's
   *  id, or the provider id. */
  key: string;
  name: string;
  /** The one-liner under the name: a role for a custom agent, the model for a
   *  bare provider. */
  desc: string;
  /** What starting a chat with this option means. */
  pick: ChatAgentPick;
  custom?: CustomAgent;
  provider?: Provider;
  /** Why this agent can't be started right now (not installed, or not
   *  container-ready under the Docker engine); null when it can. */
  disabled: string | null;
  /** Its short status word — the probed CLI version, or the refusal in brief. */
  note: string;
}

/** The agents this surface offers, in two groups.
 *
 *  The custom group is narrowed to the Project Manager on purpose: the rest of
 *  the library is built to *write code*, and these chats are denied the publish
 *  ops backend-side, so offering a tester or an architect here is an offer the
 *  surface can't honour. The bare coding agents stay — talking to Claude or
 *  Codex about the roadmap is a real thing to want — but they sit second, as
 *  the fallback they are. */
export function useAgentOptions(defaultAgentId: string | undefined): {
  custom: AgentOption[];
  providers: AgentOption[];
  /** Every option in display order, which is also the ⌘1…⌘9 order. */
  all: AgentOption[];
} {
  const customAgents = useAppStore((s) => s.customAgents);
  const providerFlags = useAppStore((s) => s.providerFlags);
  const availability = useAgentAvailability();

  const pm = customAgents.find((a) => a.id === defaultAgentId);
  const custom: AgentOption[] =
    pm && providerFlags[pm.base] !== false
      ? [
          {
            key: pm.id,
            name: pm.name,
            desc: pm.description || providerLabel(pm.base),
            // The preset's model comes from the agent definition — as does its
            // effort — so its card offers no model of its own.
            pick: { provider: pm.base, model: pm.model ?? undefined, customAgentId: pm.id },
            custom: pm,
            // A custom agent inherits its base provider's availability exactly.
            disabled: availability(pm.base).reason,
            note: availability(pm.base).note,
          },
        ]
      : [];

  const providers: AgentOption[] = PROVIDERS.filter((p) => providerFlags[p.id] !== false).map(
    (p) => {
      const { reason, note } = availability(p.id);
      return {
        key: p.id,
        name: p.label,
        // Unselected, a provider card carries the same status word the composer's
        // picker shows — the probed version. Selecting it swaps in the model it
        // will run, which is the only thing that distinguishes two chats with the
        // same provider (see AgentCard).
        desc: p.fixedModel ? "Manages its own model" : note,
        pick: { provider: p.id },
        provider: p,
        disabled: reason,
        note,
      };
    },
  );

  return { custom, providers, all: [...custom, ...providers] };
}
