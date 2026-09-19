import type { AgentRecord } from "@desktop/api/types/agent";
import { type MutableRefObject, useEffect, useMemo } from "react";
import { applyPolicy, type ChatItem, getAdapter } from "../../adapters";
import { isBusy, providerLabel } from "../../lib/agents";
import { fmtElapsed, useElapsed } from "../../lib/hooks";
import { useStickyScroll } from "../../lib/useStickyScroll";
import { useStore } from "../../store";
import { ApprovalCard, ErrorCard, PublishApprovalCard } from "./ApprovalCard";
import { ProposalCard } from "./ProposalCard";
import { SubagentStrip } from "./SubagentStrip";
import { type TasksByToolUse, Transcript } from "./Transcript";

export function ChatTab({
  agent,
  pinRef,
}: {
  agent: AgentRecord;
  /** Owned by the screen so the composer can re-pin on send. */
  pinRef?: MutableRefObject<boolean>;
}) {
  // No `?? []` inside a selector: a fresh empty value on every call is a new
  // reference and re-renders forever under zustand v5.
  const log = useStore((s) => s.logs[agent.id]);
  const pending = useStore((s) => s.pendingToolUse[agent.id]);
  const startedAt = useStore((s) => s.turnStartedAt[agent.id]);
  const tasks = useStore((s) => s.backgroundTasks[agent.id]);
  // Selected whole and filtered below: a selector that filters returns a fresh
  // array every call, which re-renders forever under zustand v5.
  const publishApprovals = useStore((s) => s.pendingPublishApprovals);
  // Indexed straight out of the store, like `log` above. Only a planning chat
  // has any — an agent's project may well have ghosts on its board, but they are
  // not a decision *this* conversation raised.
  const proposals = useStore((s) => s.proposals[agent.project_id]);
  const loadProposals = useStore((s) => s.loadProposals);
  const connected = useStore((s) => s.connection === "connected");
  const planning = !!agent.purpose;
  const busy = isBusy(agent);
  const elapsed = useElapsed(startedAt, busy);

  // The ghosts can predate this session — a chat resumed tomorrow has to show
  // what the PM proposed today — and nothing replays the events that created
  // them, so the board is read on arrival and again on every reconnect.
  useEffect(() => {
    if (planning && connected) void loadProposals(agent.project_id);
  }, [planning, connected, agent.project_id, loadProposals]);

  const visible = useMemo(
    () => applyPolicy(log ?? [], getAdapter(agent.provider).policy),
    [log, agent.provider],
  );
  const pendingIds = Object.keys(pending ?? {});
  const publishForAgent = useMemo(
    () => publishApprovals.filter((r) => r.agent_id === agent.id),
    [publishApprovals, agent.id],
  );
  const callById = useMemo(() => {
    const map = new Map<string, Extract<ChatItem, { kind: "tool_call" }>>();
    for (const it of log ?? []) if (it.kind === "tool_call") map.set(it.id, it);
    return map;
  }, [log]);
  // A task's `toolUseId` is the id of the Agent/Bash tool_call that launched
  // it — the key a tool row looks itself up by.
  const tasksByToolUse = useMemo(() => {
    const map: TasksByToolUse = {};
    for (const t of Object.values(tasks ?? {})) if (t.toolUseId) map[t.toolUseId] = t;
    return map;
  }, [tasks]);

  // `log` is the signal that matters: a streaming message is extended in
  // place, so the item count stays put while the rendered height grows. The
  // rest are the cards this pane draws as siblings of the log — the approval
  // prompt, the error card, the working indicator.
  const { ref: scroller, onScroll } = useStickyScroll<HTMLDivElement>(
    [log, pending, publishForAgent, proposals, agent.status, busy],
    pinRef,
  );

  const jumpTo = (toolUseId: string) =>
    scroller.current
      ?.querySelector(`[data-tool-use-id="${toolUseId}"]`)
      ?.scrollIntoView({ behavior: "smooth", block: "center" });

  return (
    <>
      <SubagentStrip tasks={tasks} onJump={jumpTo} />
      <div className="scroll chat" ref={scroller} onScroll={onScroll}>
        <Transcript items={visible} tasks={tasksByToolUse} />
        {pendingIds.map((toolUseId) => (
          <ApprovalCard
            key={toolUseId}
            agentId={agent.id}
            provider={agent.provider}
            toolUseId={toolUseId}
            call={callById.get(toolUseId)}
          />
        ))}
        {publishForAgent.map((request) => (
          <PublishApprovalCard key={request.id} request={request} />
        ))}
        {planning && proposals?.map((item) => <ProposalCard key={item.id} item={item} />)}
        {agent.status === "error" && (
          <ErrorCard agentId={agent.id} message={agent.last_error ?? null} />
        )}
        {busy && pendingIds.length === 0 && (
          <div className="working rise">
            <span className="working-dots">
              <i />
              <i />
              <i />
            </span>
            {providerLabel(agent.provider)} is working
            {startedAt && <span className="el">{fmtElapsed(elapsed)}</span>}
          </div>
        )}
        {visible.length === 0 && !busy && (
          <div className="empty">
            <b>No conversation yet</b>
            Send the first message below.
          </div>
        )}
      </div>
    </>
  );
}
