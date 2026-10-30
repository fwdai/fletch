import type { AgentRecord } from "@/api";
import { Icon } from "@/components/Icon";
import { useAppStore } from "@/store";

/** The one action a box agent's popover offers for its box, if any. While the
 *  box is up (its registry keeps it up until the idle timer fires) the user
 *  can stop it without archiving the workspace. When it failed for want of
 *  its tools disk, whose build failed and now waits out a backoff before the
 *  next attempt, Retry builds it now. */
export function BoxActions({ agent }: { agent: AgentRecord }) {
  const boxState = useAppStore((s) => s.boxStates[agent.id]);
  const tools = useAppStore((s) => s.boxTools[agent.provider]);
  const stopBox = useAppStore((s) => s.stopBox);
  const retryTools = useAppStore((s) => s.retryBoxTools);
  const boxUp = boxState?.phase === "running" || boxState?.phase === "starting";
  const toolsFailed = boxState?.phase === "failed" && tools?.phase === "failed";
  if (!boxUp && !toolsFailed) return null;

  return (
    <div className="ws-pop-actions">
      {boxUp && (
        <button type="button" className="ws-act" onClick={() => void stopBox(agent.id)}>
          <Icon name="stop" size={12} /> Stop box
        </button>
      )}
      {toolsFailed && (
        <button type="button" className="ws-act" onClick={() => void retryTools(agent.provider)}>
          <Icon name="refresh" size={12} /> Retry tools install
        </button>
      )}
    </div>
  );
}
