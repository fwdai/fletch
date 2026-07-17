import type { AgentRecord } from "@/api";
import { ProviderIcon } from "@/components/ProviderIcon";
import { Mono } from "@/components/SettingsScreen/CustomAgents/Mono";
import { providerChip, providerLabel } from "@/data/providers";
import { useAppStore } from "@/store";

/** The agent's identity chip: a custom agent's colored monogram (its `color`
 *  hue + name initials), or the base provider glyph for a built-in spawn. Shared
 *  by the sidebar row and Mission Control so both read the same identity — the
 *  custom-agent lookup + provider fallback lives here once, not per call-site. */
export function AgentIdentityChip({ agent, size = 14 }: { agent: AgentRecord; size?: number }) {
  const customAgent = useAppStore((s) =>
    agent.custom_agent_id ? s.customAgents.find((a) => a.id === agent.custom_agent_id) : undefined,
  );
  return (
    <span
      className="ag-prov-chip tip"
      data-tip={
        customAgent
          ? `${customAgent.name} · ${providerLabel(agent.provider)}`
          : providerLabel(agent.provider)
      }
      data-tip-down=""
    >
      {customAgent ? (
        <Mono name={customAgent.name} hue={customAgent.color} size={size} />
      ) : (
        <ProviderIcon slug={agent.provider} {...providerChip(agent.provider)} size={size} />
      )}
    </span>
  );
}
