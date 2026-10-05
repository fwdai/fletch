// Where the composer would be. A sub-agent takes its direction from the agent
// that launched it, so this says so and offers the way back to that agent's
// composer. When sending to a sub-agent is supported, a composer slots in
// here — the thread's layout already reserves the spot.

import type { AgentRecord } from "@/api";
import { Icon } from "@/components/Icon";
import { Button } from "@/components/ui/Button";
import { useAppStore } from "@/store";
import type { ThreadState } from "./thread";

export function ThreadFooter({ agent, state }: { agent: AgentRecord; state: ThreadState }) {
  const close = useAppStore((s) => s.closeSubagentThread);
  return (
    <div className="thread-foot flex-center">
      <Icon name="subagent" size={12} className="thread-foot-icon" />
      <span className="thread-foot-text">
        {state === "running" ? "Live thread, updating as the sub-agent works. " : "Thread ended. "}
        Sub-agents take direction from <b>{agent.title || agent.name}</b>, not from here.
      </span>
      <Button variant="outline" size="sm" onClick={close}>
        Reply in conversation
      </Button>
    </div>
  );
}
