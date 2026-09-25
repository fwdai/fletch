import type { AgentRecord } from "@/api";
import { ProviderIcon } from "@/components/ProviderIcon";
import { Mono } from "@/components/SettingsScreen/CustomAgents/Mono";
import { lookupModel } from "@/data/modelCatalog";
import { effortLabel } from "@/data/providerDetail";
import { providerChip, providerLabel } from "@/data/providers";
import { useAppStore } from "@/store";

/** The agent's identity chip: a custom agent's colored monogram (its `color`
 *  hue + name initials), or the base provider glyph for a built-in spawn. Shared
 *  by the sidebar row and Mission Control so both read the same identity — the
 *  custom-agent lookup + provider fallback lives here once, not per call-site.
 *
 *  The glyph already says which agent it is, so the tooltip shows what it
 *  can't: the model and, for claude, the effort ("Claude Fable 5.1 · High").
 *  The model is the one chosen at spawn, else the one the transcript last
 *  reported; until either is known the tip falls back to the agent's name. */
export function AgentIdentityChip({ agent, size = 14 }: { agent: AgentRecord; size?: number }) {
  const customAgent = useAppStore((s) =>
    agent.custom_agent_id ? s.customAgents.find((a) => a.id === agent.custom_agent_id) : undefined,
  );
  const catalog = useAppStore((s) => s.modelCatalog);
  const liveModel = useAppStore((s) => s.usage[agent.id]?.context.model);
  const modelId = agent.model ?? liveModel;
  const tip = [
    customAgent?.name,
    modelId ? (lookupModel(catalog, modelId)?.name ?? modelId) : providerLabel(agent.provider),
    agent.effort && effortLabel(agent.effort),
  ]
    .filter(Boolean)
    .join(" · ");
  return (
    <span className="ag-prov-chip tip" data-tip={tip}>
      {customAgent ? (
        <Mono name={customAgent.name} hue={customAgent.color} size={size} />
      ) : (
        <ProviderIcon slug={agent.provider} {...providerChip(agent.provider)} size={size} />
      )}
    </span>
  );
}
