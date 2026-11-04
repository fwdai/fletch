import type { AgentRecord } from "@/api";
import { Icon } from "@/components/Icon";
import { useAppStore } from "@/store";
import { useGate } from "@/store/capabilities";
import { boxIsUp } from "./isolation";

/** The one action a box agent's popover offers for its box, if any, as a
 *  button for the popover's action row. While the box is up (its registry
 *  keeps it up until the idle timer fires) the user can stop it without
 *  archiving the workspace. When it failed for want of its tools disk, whose
 *  build failed and now waits out a backoff before the next attempt, Retry
 *  builds it now. */
export function BoxActions({ agent }: { agent: AgentRecord }) {
  const boxState = useAppStore((s) => s.boxStates[agent.id]);
  const tools = useAppStore((s) => s.boxTools[agent.provider]);
  const stopBox = useAppStore((s) => s.stopBox);
  const retryTools = useAppStore((s) => s.retryBoxTools);
  const stopGate = useGate("stopVm");
  const setupGate = useGate("vmSetup");
  const boxUp = boxIsUp(boxState?.phase);
  const toolsFailed = boxState?.phase === "failed" && tools?.phase === "failed";

  if (boxUp && !stopGate) {
    return (
      <button type="button" className="ws-act" onClick={() => void stopBox(agent.id)}>
        <Icon name="stop" size={12} /> Stop box
      </button>
    );
  }
  if (toolsFailed && !setupGate) {
    return (
      <button type="button" className="ws-act" onClick={() => void retryTools(agent.provider)}>
        <Icon name="refresh" size={12} /> Retry tools install
      </button>
    );
  }
  return null;
}
