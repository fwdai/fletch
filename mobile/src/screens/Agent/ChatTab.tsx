import type { AgentRecord } from "@desktop/api/types/agent";
import { type MutableRefObject, useEffect, useMemo } from "react";
import { applyPolicy, type ChatItem, getAdapter } from "../../adapters";
import { isAgentBusy, providerLabel } from "../../lib/agents";
import { fmtElapsed, useElapsed } from "../../lib/hooks";
import { ignore } from "../../lib/ignore";
import { tasksByToolUse } from "../../lib/thread";
import { useStickyScroll } from "../../lib/useStickyScroll";
import { useStore } from "../../store";
import { ApprovalCard, ErrorCard, PublishApprovalCard } from "./ApprovalCard";
import { LoadOlder } from "./LoadOlder";
import { LogPlaceholder } from "./LogState";
import { ProposalCard } from "./ProposalCard";
import { SubagentStrip } from "./SubagentStrip";
import { Transcript } from "./Transcript";

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
  const load = useStore((s) => s.logLoads[agent.id]);
  const loadAgent = useStore((s) => s.loadAgent);
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
  const push = useStore((s) => s.push);
  const send = useStore((s) => s.send);
  const connected = useStore((s) => s.connection === "connected");
  const planning = !!agent.purpose;
  const busy = useStore((s) => isAgentBusy(s, agent));
  const elapsed = useElapsed(startedAt, busy);

  // The ghosts can predate this session — a chat resumed tomorrow has to show
  // what the PM proposed today — and nothing replays the events that created
  // them, so the board is read on arrival and again on every reconnect.
  useEffect(() => {
    if (planning && connected) void loadProposals(agent.project_id);
  }, [planning, connected, agent.project_id, loadProposals]);

  // `openAgent` reads the history as it pushes this screen. Any other way in
  // (a spawn whose first send failed and dropped its optimistic turn, say)
  // would otherwise sit on the skeleton with no read behind it.
  const unread = log === undefined && load === undefined;
  useEffect(() => {
    if (unread && connected) void loadAgent(agent.id).catch(ignore);
  }, [unread, connected, agent.id, loadAgent]);

  const policy = getAdapter(agent.provider).policy;
  const visible = useMemo(() => applyPolicy(log ?? [], policy), [log, policy]);
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
  const byToolUse = useMemo(() => tasksByToolUse(tasks), [tasks]);
  // Nothing to draw yet. What is drawn instead depends on the read
  // (LogPlaceholder); while it is still out, the skeleton stands in for the
  // working line too.
  const empty = visible.length === 0;
  const loading = empty && (load === undefined || load.status === "loading");

  // `log` is the signal that matters: a streaming message is extended in
  // place, so the item count stays put while the rendered height grows. The
  // rest are the cards this pane draws as siblings of the log — the approval
  // prompt, the error card, the working indicator.
  const { ref: scroller, onScroll } = useStickyScroll<HTMLDivElement>(
    [log, pending, publishForAgent, proposals, agent.status, busy],
    pinRef,
  );

  // A sub-agent's thread is its own screen (screens/Subagent), reached from the
  // strip and from its card in the log alike.
  const openThread = (toolUseId: string) =>
    push("subagent", { agentId: agent.id, path: toolUseId });
  const resend = (text: string, attachments?: string[]) =>
    void send(agent.id, text, attachments).catch(ignore);

  return (
    <>
      <SubagentStrip tasks={tasks} onOpen={openThread} />
      <div className="scroll chat" ref={scroller} onScroll={onScroll}>
        <LoadOlder agentId={agent.id} scroller={scroller} />
        <Transcript
          items={visible}
          tasks={byToolUse}
          busy={busy}
          openThread={openThread}
          resend={resend}
        />
        {empty && <LogPlaceholder agentId={agent.id} load={load} busy={busy} />}
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
        {busy && pendingIds.length === 0 && !loading && (
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
      </div>
    </>
  );
}
