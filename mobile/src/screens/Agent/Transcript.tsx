// The chat log as blocks: messages, notices and cards of tool rows. A tool row
// that launched a sub-agent holds that sub-agent's own turns as `children`,
// rendered with this same component under the row — which is why the row and
// the log live in one module.

import type { BackgroundTask } from "@desktop/adapters/shared/backgroundTasks";
import { Icon } from "@desktop/components/Icon";
import { useMemo, useState } from "react";
import type { ChatItem } from "../../adapters";
import { SentChips } from "../../attachments";
import { Md } from "../../components/Md";
import { resultSummary, resultText, TOOL_HUE, TOOL_ICON, toolArg } from "../../lib/tools";

type ToolCall = Extract<ChatItem, { kind: "tool_call" }>;
type ToolResult = Extract<ChatItem, { kind: "tool_result" }>;

/** Background tasks by the tool_use id that launched them. */
export type TasksByToolUse = Record<string, BackgroundTask>;

type Block =
  | { kind: "item"; key: string; item: ChatItem }
  | { kind: "tools"; key: string; calls: { call: ToolCall; result: ToolResult | null }[] };

/** Group the flat item list the way the design does: consecutive tool calls in
 *  one card, each paired with its result, and results never rendered alone. */
function toBlocks(items: ChatItem[]): Block[] {
  const results = new Map<string, ToolResult>();
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
      return (
        <div className="msg-user rise">
          {item.text}
          <SentChips paths={item.attachments} />
        </div>
      );
    case "queued_message":
      return (
        <div className="msg-user queued rise">
          {item.text}
          <SentChips paths={item.attachments} />
        </div>
      );
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

const Dots = () => (
  <span className="working-dots">
    <i />
    <i />
    <i />
  </span>
);

/** One tool call. Runs (dots) while it streams or while the background task
 *  it launched is still going; reads as failed when that task did. Expands to
 *  the sub-agent's threaded turns and/or the raw result. */
function ToolRow({
  call,
  result,
  tasks,
}: {
  call: ToolCall;
  result: ToolResult | null;
  tasks?: TasksByToolUse;
}) {
  const [open, setOpen] = useState(false);
  const out = result ? resultText(result.content) : "";
  const task = tasks?.[call.id];
  const children = call.children ?? [];
  const running = !!call.streaming || task?.status === "running";
  const failed = task?.status === "failed";
  const expandable = !!out || children.length > 0;
  const tone = TOOL_HUE[call.name] ?? "var(--fg-2)";
  return (
    <div className="tool-wrap" data-tool-use-id={call.id}>
      <button
        type="button"
        className={`tool${open ? " open" : ""}${failed ? " failed" : ""}`}
        style={{ "--tool": tone } as React.CSSProperties}
        onClick={() => expandable && setOpen((o) => !o)}
      >
        <span className="ic">
          <Icon name={TOOL_ICON[call.name] ?? "wrench"} size={13} />
        </span>
        <span className="nm">{call.name}</span>
        <span className="arg">{toolArg(call.input)}</span>
        {running ? (
          <span className="mt run">
            <Dots />
          </span>
        ) : failed ? (
          <span className="mt err">{task.failureStatus ?? "failed"}</span>
        ) : (
          out && <span className={`mt${result?.is_error ? " err" : ""}`}>{resultSummary(out)}</span>
        )}
        {expandable && <Icon name="chevR" size={14} className="chev" />}
      </button>
      {open && children.length > 0 && (
        <div className="tool-sub">
          <Transcript items={children} tasks={tasks} />
        </div>
      )}
      {open && out && <div className="tool-out">{out}</div>}
    </div>
  );
}

export function Transcript({ items, tasks }: { items: ChatItem[]; tasks?: TasksByToolUse }) {
  const blocks = useMemo(() => toBlocks(items), [items]);
  return (
    <>
      {blocks.map((b) =>
        b.kind === "tools" ? (
          <div key={b.key} className="tools rise">
            {b.calls.map(({ call, result }) => (
              <ToolRow key={call.id} call={call} result={result} tasks={tasks} />
            ))}
          </div>
        ) : (
          <Item key={b.key} item={b.item} />
        ),
      )}
    </>
  );
}
