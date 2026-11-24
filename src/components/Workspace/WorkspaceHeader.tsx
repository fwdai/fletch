import type { AgentRecord } from "@/api";
import { ContextBadge } from "@/components/ContextBadge";
import { PanelToggle } from "@/components/PanelToggle";
import { agentTitle } from "@/util/agentTitle";

/** Header strip above the workspace body: the conversation's own line. Houses
 *  the left-sidebar toggle, the conversation's title, and what belongs to the
 *  conversation as a whole — the context badge — then the right-panel toggle. Where the conversation runs (project,
 *  workspace, git) is the title bar's to say, and whether the agent is working
 *  is said by the sidebar row, the title bar and the composer. A sub-agent
 *  thread's header (SubagentThread/ThreadHeader) keeps this exact skeleton —
 *  one line, led by the title — so stepping into and out of a thread moves
 *  nothing but the words. */
interface Props {
  agent: AgentRecord;
}

export function WorkspaceHeader({ agent }: Props) {
  return (
    <div className="center-h flex-center">
      <PanelToggle side="left" />

      <div className="task">
        <div className="t-name">
          <span title={agentTitle(agent)}>{agentTitle(agent)}</span>
        </div>
      </div>

      {/* Forking lives under each message (TurnFooter), where it has a point
          to fork from; the header carries what the conversation is using. */}
      <ContextBadge agent={agent} />

      <PanelToggle side="right" />
    </div>
  );
}
