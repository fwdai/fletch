import { useEffect, useRef, useState } from "react";
import { ask } from "@tauri-apps/plugin-dialog";
import { type AgentRecord } from "../api";
import { type ManagedItem, useAppStore } from "../store";
import { ViewToggle } from "./ViewToggle";

const EMPTY_LOG: ManagedItem[] = [];

/** Custom view: structured chat UI rendered from claude's stream-json
 *  events. The user types into our textarea; we ship the message via
 *  the backend `send_user_message` command (which writes a JSON
 *  envelope to claude's stdin). */
export function CustomAgentView({ agent }: { agent: AgentRecord }) {
  const log = useAppStore((s) => s.managedLogs[agent.id] ?? EMPTY_LOG);
  const busy = useAppStore((s) => s.managedBusy[agent.id] ?? false);
  const send = useAppStore((s) => s.sendUserMessage);
  const stop = useAppStore((s) => s.stop);
  const discard = useAppStore((s) => s.discard);

  const [draft, setDraft] = useState("");
  const scrollRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    el.scrollTop = el.scrollHeight;
  }, [log]);

  async function onSubmit(e: React.FormEvent) {
    e.preventDefault();
    const text = draft.trim();
    if (!text) return;
    setDraft("");
    await send(agent.id, text);
  }

  async function onStop() {
    const ok = await ask(
      `Stop agent "${agent.name}"? The process will be terminated.`,
      { title: "Stop agent", kind: "warning" },
    );
    if (ok) await stop(agent.id);
  }

  async function onDiscard() {
    const ok = await ask(
      `Remove "${agent.name}"?\n\nThis will delete:\n` +
        `  • the worktree at .worktrees/${agent.id} (any uncommitted work)\n` +
        `  • the branch ${agent.branch}\n\n` +
        `Branch deletion can be undone via git reflog within ~90 days.`,
      { title: "Remove agent", kind: "warning" },
    );
    if (ok) await discard(agent.id);
  }

  const canSend =
    !busy && (agent.status === "running" || agent.status === "idle");

  return (
    <div className="termwrap">
      <div className="termheader">
        <div className="left">
          <span className="name">{agent.name}</span>
          <span className="branch">{agent.branch}</span>
          <span className="status" data-status={agent.status}>
            {agent.status}
          </span>
        </div>
        <div className="right">
          {/* Disable the toggle while a turn is in flight — switching
              tears down the process, which would truncate the response. */}
          <ViewToggle agentId={agent.id} current="custom" disabled={busy} />
          {(agent.status === "running" || agent.status === "spawning") && (
            <button onClick={onStop}>Stop</button>
          )}
          <button onClick={onDiscard}>Remove</button>
        </div>
      </div>
      {agent.last_error && <div className="errbar">{agent.last_error}</div>}

      <div className="msglog" ref={scrollRef}>
        {log.length === 0 && (
          <div className="msgempty">
            {canSend
              ? "Send a message to begin."
              : "Waiting for claude to start…"}
          </div>
        )}
        {log.map((item, i) => (
          <MessageItem key={i} item={item} />
        ))}
        {busy && (
          <div className="msgbusy">
            <span className="dots">
              <span />
              <span />
              <span />
            </span>
            claude is thinking…
          </div>
        )}
      </div>

      <form className="msginput" onSubmit={onSubmit}>
        <textarea
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
              e.preventDefault();
              onSubmit(e as unknown as React.FormEvent);
            }
          }}
          placeholder={
            canSend
              ? "Reply to claude — ⌘↵ to send"
              : busy
                ? "Waiting for claude…"
                : "Agent is not running"
          }
          rows={2}
          disabled={!canSend}
        />
        <button
          type="submit"
          className="primary"
          disabled={!canSend || !draft.trim()}
        >
          Send
        </button>
      </form>
    </div>
  );
}

function MessageItem({ item }: { item: ManagedItem }) {
  switch (item.kind) {
    case "user":
      return (
        <div className="msg msg-user">
          <div className="msg-role">You</div>
          <div className="msg-body">{item.text}</div>
        </div>
      );
    case "assistant":
      return (
        <div className="msg msg-assistant">
          <div className="msg-role">Claude {item.streaming ? "…" : ""}</div>
          <div className="msg-body">{item.text}</div>
        </div>
      );
    case "tool_use":
      return <ToolUseItem item={item} />;
    case "tool_result":
      return <ToolResultItem item={item} />;
    case "result":
      return (
        <div className={`msg msg-result${item.is_error ? " error" : ""}`}>
          <div className="msg-body">{item.text}</div>
        </div>
      );
    case "system":
      return null;
  }
}

function ToolUseItem({
  item,
}: {
  item: Extract<ManagedItem, { kind: "tool_use" }>;
}) {
  const [open, setOpen] = useState(false);
  return (
    <div className="msg msg-tool">
      <button
        className="tool-header"
        onClick={() => setOpen((o) => !o)}
        type="button"
      >
        <span className="caret">{open ? "▾" : "▸"}</span>
        <span className="tool-name">⚙ {item.name}</span>
        <span className="tool-summary">{summarize(item.input)}</span>
      </button>
      {open && (
        <pre className="tool-body">
          {JSON.stringify(item.input, null, 2)}
        </pre>
      )}
    </div>
  );
}

function ToolResultItem({
  item,
}: {
  item: Extract<ManagedItem, { kind: "tool_result" }>;
}) {
  const [open, setOpen] = useState(false);
  const text = renderToolResult(item.content);
  return (
    <div className={`msg msg-tool-result${item.is_error ? " error" : ""}`}>
      <button
        className="tool-header"
        onClick={() => setOpen((o) => !o)}
        type="button"
      >
        <span className="caret">{open ? "▾" : "▸"}</span>
        <span className="tool-name">↳ result</span>
        <span className="tool-summary">{firstLine(text)}</span>
      </button>
      {open && <pre className="tool-body">{text}</pre>}
    </div>
  );
}

function summarize(input: unknown): string {
  if (input == null) return "";
  if (typeof input === "string") return firstLine(input);
  try {
    return firstLine(JSON.stringify(input));
  } catch {
    return "";
  }
}

function firstLine(s: string): string {
  const idx = s.indexOf("\n");
  const head = idx === -1 ? s : s.slice(0, idx);
  return head.length > 120 ? head.slice(0, 117) + "…" : head;
}

function renderToolResult(content: unknown): string {
  if (typeof content === "string") return content;
  if (Array.isArray(content)) {
    return content
      .map((block) => {
        if (block && typeof block === "object" && "text" in block) {
          return String((block as { text: unknown }).text ?? "");
        }
        return JSON.stringify(block);
      })
      .join("\n");
  }
  return JSON.stringify(content, null, 2);
}
