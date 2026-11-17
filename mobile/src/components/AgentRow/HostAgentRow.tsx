import { OnHost } from "../../host/OnHost";
import type { HostAgent } from "../../lib/agents";
import { severalHosts, useApp, useHostName, useHostStore } from "../../store";
import { AgentRow } from ".";

/** `agent` of `hostKey`'s host, in a list several hosts' agents can share: the
 *  row reads that host's live state, names the host while more than one is
 *  paired, and opens the agent there. Key a list of them with `hostAgentKey`. */
export function HostAgentRow({ hostKey, agent }: HostAgent) {
  return (
    <OnHost hostKey={hostKey}>
      <Row agent={agent} />
    </OnHost>
  );
}

function Row({ agent }: Pick<HostAgent, "agent">) {
  const openAgent = useHostStore((s) => s.openAgent);
  const several = useApp(severalHosts);
  const host = useHostName();
  return (
    <AgentRow
      agent={agent}
      host={several ? host : null}
      onClick={(e) => {
        // A row on a project card opens the agent, not the project as well.
        e.stopPropagation();
        openAgent(agent.id);
      }}
    />
  );
}
