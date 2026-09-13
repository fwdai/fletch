import { Icon } from "@desktop/components/Icon";
import type { ChatItem } from "../../adapters";
import { providerLabel } from "../../lib/agents";
import { ignore } from "../../lib/ignore";
import { toolArg } from "../../lib/tools";
import { useStore } from "../../store";

type ToolCall = Extract<ChatItem, { kind: "tool_call" }>;

/** The agent is genuinely suspended on a held `can_use_tool` prompt: answering
 *  returns the (possibly unchanged) input as the tool result and resumes the
 *  turn. `updatedInput` is the call's own input — the phone never rewrites what
 *  the agent asked to run. */
export function ApprovalCard({
  agentId,
  provider,
  toolUseId,
  call,
}: {
  agentId: string;
  provider: string;
  toolUseId: string;
  call: ToolCall | undefined;
}) {
  const answerToolUse = useStore((s) => s.answerToolUse);
  const answer = (behavior: "allow" | "deny") =>
    void answerToolUse(agentId, toolUseId, call?.input ?? {}, behavior).catch(ignore);
  return (
    <div className="appr rise">
      <div className="h">
        <Icon name="alert" size={16} />
        Approval needed
      </div>
      <div className="why">
        {providerLabel(provider)} wants to run {call ? call.name : "a tool"} outside the sandbox.
      </div>
      {call && <div className="cmd">{toolArg(call.input)}</div>}
      <div className="acts">
        <button type="button" className="btn ghost" onClick={() => answer("deny")}>
          Deny
        </button>
        <button type="button" className="btn primary" onClick={() => answer("allow")}>
          <Icon name="check" size={15} strokeWidth={2.2} />
          Allow once
        </button>
      </div>
      <div className="always">Answering resumes the turn on the host.</div>
    </div>
  );
}

/** An errored run: `resume_agent` picks the session back up. */
export function ErrorCard({ agentId, message }: { agentId: string; message: string | null }) {
  const resume = useStore((s) => s.resume);
  return (
    <div className="appr err rise">
      <div className="h">
        <Icon name="alert" size={16} />
        Run paused
      </div>
      <div className="why">{message ?? "The agent stopped with an error."}</div>
      <div className="acts">
        <button
          type="button"
          className="btn primary"
          onClick={() => void resume(agentId).catch(ignore)}
        >
          <Icon name="refresh" size={15} />
          Resume
        </button>
      </div>
    </div>
  );
}
