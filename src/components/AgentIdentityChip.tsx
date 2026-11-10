import type { AgentRecord } from "@/api";
import { ProviderIcon } from "@/components/ProviderIcon";
import { Mono } from "@/components/SettingsScreen/CustomAgents/Mono";
import { lookupModel } from "@/data/modelCatalog";
import { effortLabel } from "@/data/providerDetail";
import { providerChip, providerLabel } from "@/data/providers";
import { useAppStore } from "@/store";

function useCustomAgent(agent: AgentRecord) {
  return useAppStore((s) =>
    agent.custom_agent_id ? s.customAgents.find((a) => a.id === agent.custom_agent_id) : undefined,
  );
}

/** What the identity glyph can't say: the custom agent's name, the model and,
 *  for claude, the effort ("Claude Fable 5.1 · High"). The model is the one
 *  chosen at spawn, else the one the transcript last reported; until either is
 *  known the label falls back to the provider's name. */
export function useAgentIdentityLabel(agent: AgentRecord): string {
  const customAgent = useCustomAgent(agent);
  const catalog = useAppStore((s) => s.modelCatalog);
  const liveModel = useAppStore((s) => s.usage[agent.id]?.context.model);
  const modelId = agent.model ?? liveModel;
  return [
    customAgent?.name,
    modelId ? (lookupModel(catalog, modelId)?.name ?? modelId) : providerLabel(agent.provider),
    agent.effort && effortLabel(agent.effort),
  ]
    .filter(Boolean)
    .join(" · ");
}

/** The agent's identity chip: a custom agent's colored monogram (its `color`
 *  hue + name initials), or the base provider glyph for a built-in spawn. Lives
 *  here so every surface reads the same identity — the custom-agent lookup +
 *  provider fallback lives here once, not per call-site.
 *
 *  The glyph already says which agent it is, so the tooltip shows the rest
 *  (`useAgentIdentityLabel`). A surface that spells the label out itself passes
 *  `tip={false}`. */
export function AgentIdentityChip({
  agent,
  size = 14,
  tip = true,
}: {
  agent: AgentRecord;
  size?: number;
  tip?: boolean;
}) {
  const customAgent = useCustomAgent(agent);
  const label = useAgentIdentityLabel(agent);
  return (
    <span className={tip ? "ag-prov-chip tip" : "ag-prov-chip"} data-tip={tip ? label : undefined}>
      {customAgent ? (
        <Mono name={customAgent.name} hue={customAgent.color} size={size} />
      ) : (
        <ProviderIcon slug={agent.provider} {...providerChip(agent.provider)} size={size} />
      )}
    </span>
  );
}
