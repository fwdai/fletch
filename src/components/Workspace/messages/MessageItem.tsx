import { useMemo } from "react";
import { applyPolicy, getAdapter } from "../../../adapters";
import type { ChatItem } from "../../../store";
import { stripInjectedInstructions } from "../../../util/instructions";
import { AttachmentList } from "../../Composer/AttachmentList";
import { Markdown } from "../../Markdown";
import { APP_ACTION_PREFIX } from "../../RightPanel/delegation";
import { pairToolItems, rowKey, type ViewItem } from "./pair";
import { getPresenter } from "./presenters";
import { ToolResultItem } from "./ToolResultItem";
import { ToolRow } from "./ToolRow";
import { UserInput } from "./UserInput";
import { isUserInputTool } from "./UserInput/parse";

/** Dispatcher for one rendered row. Accepts either a raw ChatItem or
 *  the derived `tool_pair` from pairToolItems(). `provider` carries the
 *  agent's adapter id down so nested subagent threads filter/pair their
 *  rows with the same display policy as the main log. `agentId` lets the
 *  user-input widget route its answer back to this agent's stdin. */
export function MessageItem({
  item,
  provider,
  agentId,
  turnId,
}: {
  item: ViewItem;
  provider?: string;
  agentId?: string;
  /** Ordinal of this user turn, used by ChatNav to locate the bubble in the
   *  DOM. Set only for top-level navigable user prompts. */
  turnId?: number;
}) {
  switch (item.kind) {
    case "user_message":
      // App-sent git-action triggers fold into a quiet chip (like slash
      // commands) instead of a user bubble — one rendering rule covers both
      // the live optimistic entry and history rebuilt from session records.
      if (item.text.startsWith(APP_ACTION_PREFIX)) {
        return (
          <div className="m-reasoning" style={{ fontStyle: "italic" }}>
            <div className="label">git action</div>
            <code>{item.text.slice(APP_ACTION_PREFIX.length)}</code>
          </div>
        );
      }
      return (
        <div className="m-user" data-chat-turn={turnId}>
          {stripInjectedInstructions(item.text)}
          {item.attachments && item.attachments.length > 0 && (
            <AttachmentList paths={item.attachments} className="message-attachments" />
          )}
        </div>
      );
    case "agent_message":
      return (
        <div className="m-agent">
          <Markdown>{item.text}</Markdown>
          {item.streaming && <span className="term-cursor" style={{ marginLeft: 4 }} />}
        </div>
      );
    case "tool_pair": {
      if (isUserInputTool(item.call.name)) {
        return (
          <UserInput
            tool={item.call.name}
            call={item.call}
            result={item.result}
            agentId={agentId}
          />
        );
      }
      const presenter = getPresenter(item.call.name);
      const children = item.call.children ?? [];
      return (
        <ToolRow
          name={item.call.name}
          icon={presenter.icon}
          isError={item.result?.is_error}
          summary={presenter.summary(item.call, item.result)}
          expanded={
            <>
              {presenter.expanded(item.call, item.result)}
              {children.length > 0 && (
                <SubagentThread items={children} provider={provider} agentId={agentId} />
              )}
            </>
          }
        />
      );
    }
    case "tool_call": {
      // Bare tool_call without pairing — happens only if the caller
      // bypasses pairToolItems(). Render through the presenter anyway,
      // with a null result.
      if (isUserInputTool(item.name)) {
        return <UserInput tool={item.name} call={item} result={null} agentId={agentId} />;
      }
      const presenter = getPresenter(item.name);
      return (
        <ToolRow
          name={item.name}
          icon={presenter.icon}
          summary={presenter.summary(item, null)}
          expanded={presenter.expanded(item, null)}
        />
      );
    }
    case "tool_result":
      return <ToolResultItem item={item} />;
    case "notice":
      return <NoticeView item={item} />;
  }
}

/** A subagent's threaded sub-conversation, rendered inside its spawning
 *  tool row's expanded body. Runs the children through the same
 *  policy → pairing → MessageItem pipeline as the main log (so tool calls
 *  pair with their results and hidden notices stay hidden), nested under a
 *  quiet left rail. Recurses for subagents that spawn their own subagents. */
function SubagentThread({
  items,
  provider,
  agentId,
}: {
  items: ChatItem[];
  provider?: string;
  agentId?: string;
}) {
  // Re-derive only when the children or provider change — not on every parent
  // re-render (e.g. a streaming token elsewhere in the main log).
  const rows = useMemo(
    () => pairToolItems(applyPolicy(items, getAdapter(provider).policy)),
    [items, provider],
  );
  if (rows.length === 0) return null;
  return (
    <div
      style={{
        marginTop: 8,
        paddingLeft: 12,
        borderLeft: "2px solid var(--accent-line)",
      }}
    >
      {rows.map((row, i) => (
        <MessageItem key={rowKey(row, i)} item={row} provider={provider} agentId={agentId} />
      ))}
    </div>
  );
}

function NoticeView({ item }: { item: Extract<ChatItem, { kind: "notice" }> }) {
  if (item.subtype === "slash_command") {
    return (
      <div className="m-reasoning" style={{ fontStyle: "italic" }}>
        <div className="label">command</div>
        <code>{item.text}</code>
      </div>
    );
  }
  if (item.subtype === "compact_summary") {
    return (
      <div className="m-reasoning" style={{ fontStyle: "italic" }}>
        <div className="label">system</div>
        {item.text}
      </div>
    );
  }
  const isError = item.is_error || item.subtype === "error";
  return (
    <div
      className="m-reasoning"
      style={{
        color: isError ? "var(--danger)" : "var(--fg-3)",
        borderLeftColor: isError ? "var(--danger)" : undefined,
      }}
    >
      <div className="label">{labelFor(item.subtype)}</div>
      {item.text}
    </div>
  );
}

function labelFor(subtype: string): string {
  switch (subtype) {
    case "error":
      return "Error";
    case "reasoning":
      return "Thinking";
    case "hook_output":
      return "Hook";
    case "turn_end":
      return "Turn";
    case "info":
      return "Info";
    default:
      return subtype;
  }
}
