import type { AgentRecord } from "@/api";
import { ProviderIcon } from "@/components/ProviderIcon";
import { Mono } from "@/components/SettingsScreen/CustomAgents/Mono";
import { agentIdentityTip } from "@/data/modelCatalog";
import { providerChip, providerLabel } from "@/data/providers";
import { useAppStore } from "@/store";

/** The agent's identity chip: a custom agent's colored monogram (its `color`
 *  hue + name initials), or the base provider glyph for a built-in spawn. Shared
 *  by the sidebar row and Mission Control so both read the same identity — the
 *  custom-agent lookup + provider fallback lives here once, not per call-site.
 *
 *  The glyph already says which agent it is, so the tooltip adds what it can't
 *  show: the model ("Claude Code · Fable 5.1") and, for claude, the session's
 *  effort level. */
export function AgentIdentityChip({ agent, size = 14 }: { agent: AgentRecord; size?: number }) {
  const customAgent = useAppStore((s) =>
    agent.custom_agent_id ? s.customAgents.find((a) => a.id === agent.custom_agent_id) : undefined,
  );
  const catalog = useAppStore((s) => s.modelCatalog);
  const liveModel = useAppStore((s) => s.usage[agent.id]?.context.model);
  const tip = agentIdentityTip({
    providerLabel: providerLabel(agent.provider),
    customAgentName: customAgent?.name,
    catalog,
    model: agent.model,
    liveModel,
    effort: agent.effort,
  });
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
