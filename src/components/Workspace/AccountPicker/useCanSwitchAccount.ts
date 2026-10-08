import type { AgentRecord } from "@/api";
import { supportsAccounts } from "@/data/providerDetail";
import { useGate } from "@/store/capabilities";

/** Whether the agent's account can be switched from here at all: its provider
 *  has accounts and the environment can both list and switch them. Whether
 *  there is another account to pick, or a turn in the way, is the picker's. */
export function useCanSwitchAccount(agent: Pick<AgentRecord, "provider">): boolean {
  const gate = useGate("switchAccount");
  return gate === null && supportsAccounts(agent.provider);
}
