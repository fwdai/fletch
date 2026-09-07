import type { AgentRecord } from "@desktop/api/types/agent";
import { useEffect, useLayoutEffect, useMemo, useRef } from "react";
import { applyPolicy, type ChatItem, getAdapter } from "../../adapters";
import { Icon } from "../../components/Icon";
import { Md } from "../../components/ui";
import { isBusy, providerLabel } from "../../lib/agents";
import { fmtElapsed, useElapsed } from "../../lib/hooks";
import { useStore } from "../../store";
import { ApprovalCard, ErrorCard } from "./ApprovalCard";
import { ToolRow } from "./ToolRow";

type Block =
  | { kind: "item"; key: string; item: ChatItem }
  | {
      kind: "tools";
      key: string;
      calls: {
        call: Extract<ChatItem, { kind: "tool_call" }>;
        result: Extract<ChatItem, { kind: "tool_result" }> | null;
      }[];
    };

/** Group the flat item list the way the design does: consecutive tool calls in
 *  one card, each paired with its result, and results never rendered alone. */
function toBlocks(items: ChatItem[]): Block[] {
  const results = new Map<string, Extract<ChatItem, { kind: "tool_result" }>>();
  for (const it of items) if (it.kind === "tool_result") results.set(it.tool_use_id, it);
  const blocks: Block[] = [];
  items.forEach((item, i) => {
    if (item.kind === "tool_result") return;
    if (item.kind === "tool_call") {
      const entry = { call: item, result: results.get(item.id) ?? null };
      const last = blocks[blocks.length - 1];
      if (last?.kind === "tools") last.calls.push(entry);
      else blocks.push({ kind: "tools", key: `t${i}`, calls: [entry] });
      return;
    }
    blocks.push({ kind: "item", key: `i${i}`, item });
  });
  return blocks;
}

function Item({ item }: { item: ChatItem }) {
  switch (item.kind) {
    case "user_message":
      return <div className="msg-user rise">{item.text}</div>;
    case "queued_message":
      return <div className="msg-user queued rise">{item.text}</div>;
    case "agent_message":
      return (
        <div className="msg-text rise">
          <Md text={item.text} />
        </div>
      );
    case "notice":
      if (item.subtype === "turn_end") {
        return (
          <div className="turn-end rise">
            <i />
            <Icon name="checkCircle" size={13} style={{ color: "var(--success)" }} />
            {item.text === "success" ? "Turn complete" : item.text}
            <i />
          </div>
        );
      }
      if (item.subtype === "reasoning") return <div className="reasoning rise">{item.text}</div>;
      return <div className={`notice rise${item.is_error ? " err" : ""}`}>{item.text}</div>;
    default:
      return null;
  }
}

export function ChatTab({ agent }: { agent: AgentRecord }) {
  // No `?? []` inside a selector: a fresh empty value on every call is a new
  // reference and re-renders forever under zustand v5.
  const log = useStore((s) => s.logs[agent.id]);
  const pending = useStore((s) => s.pendingToolUse[agent.id]);
  const startedAt = useStore((s) => s.turnStartedAt[agent.id]);
  const scroller = useRef<HTMLDivElement>(null);
  const busy = isBusy(agent);
  const elapsed = useElapsed(startedAt, busy);

  const visible = useMemo(
    () => applyPolicy(log ?? [], getAdapter(agent.provider).policy),
    [log, agent.provider],
  );
  const blocks = useMemo(() => toBlocks(visible), [visible]);
  const pendingIds = Object.keys(pending ?? {});
  const callById = useMemo(() => {
    const map = new Map<string, Extract<ChatItem, { kind: "tool_call" }>>();
    for (const it of log ?? []) if (it.kind === "tool_call") map.set(it.id, it);
    return map;
  }, [log]);

  useLayoutEffect(() => {
    const el = scroller.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, []);
  const blockCount = blocks.length;
  useEffect(() => {
    const el = scroller.current;
    if (!el || blockCount === 0) return;
    // Animate while a turn is streaming; jump when the log is rebuilt from
    // records, where a long smooth scroll would look like a glitch.
    el.scrollTo({
      top: el.scrollHeight,
      behavior: agent.status === "running" ? "smooth" : "auto",
    });
  }, [blockCount, agent.status]);

  return (
    <div className="scroll chat" ref={scroller}>
      {blocks.map((b) =>
        b.kind === "tools" ? (
          <div key={b.key} className="tools rise">
            {b.calls.map(({ call, result }) => (
              <ToolRow key={call.id} call={call} result={result} />
            ))}
          </div>
        ) : (
          <Item key={b.key} item={b.item} />
        ),
      )}
      {pendingIds.map((toolUseId) => (
        <ApprovalCard
          key={toolUseId}
          agentId={agent.id}
          provider={agent.provider}
          toolUseId={toolUseId}
          call={callById.get(toolUseId)}
        />
      ))}
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
      {blocks.length === 0 && !busy && (
        <div className="empty">
          <b>No conversation yet</b>
          Send the first message below.
        </div>
      )}
    </div>
  );
}
