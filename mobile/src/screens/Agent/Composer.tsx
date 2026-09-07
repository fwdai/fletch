import type { AgentRecord } from "@desktop/api/types/agent";
import { useRef, useState } from "react";
import { Icon } from "../../components/Icon";
import { ProviderMark } from "../../components/ui";
import { isBusy, modelLabel } from "../../lib/agents";
import { autosize } from "../../lib/autosize";
import { ignore } from "../../lib/ignore";
import { useStore } from "../../store";

export function Composer({ agent }: { agent: AgentRecord }) {
  const send = useStore((s) => s.send);
  const stop = useStore((s) => s.stop);
  const openSheet = useStore((s) => s.openSheet);
  const [text, setText] = useState("");
  const ta = useRef<HTMLTextAreaElement>(null);
  const busy = isBusy(agent);

  const submit = () => {
    const value = text.trim();
    if (!value) return;
    setText("");
    requestAnimationFrame(() => autosize(ta.current));
    void send(agent.id, value).catch(ignore);
  };

  return (
    <div className="composer">
      <div className="cmp">
        <textarea
          ref={ta}
          rows={1}
          placeholder={busy ? "Message agent · queued until it pauses" : "Message agent"}
          value={text}
          onChange={(e) => {
            setText(e.target.value);
            autosize(e.currentTarget);
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault();
              submit();
            }
          }}
        />
        <div className="cmp-bar">
          <button
            type="button"
            className="mdl"
            onClick={() => openSheet("modelPicker", { agentId: agent.id })}
          >
            <ProviderMark id={agent.provider} />
            <span>{modelLabel(agent.model)}</span>
            <span className="sep">·</span>
            <span>{agent.effort ?? "default"}</span>
            <Icon name="chevD" size={12} style={{ color: "var(--fg-3)" }} />
          </button>
          <span className="grow" />
          {busy && !text.trim() ? (
            <button
              type="button"
              className="sendbtn stop"
              onClick={() => void stop(agent.id).catch(ignore)}
              aria-label="Stop agent"
            >
              <Icon name="stop" size={14} />
            </button>
          ) : (
            <button
              type="button"
              className="sendbtn"
              disabled={!text.trim()}
              onClick={submit}
              aria-label="Send"
            >
              <Icon name="arrowUp" size={17} sw={2.2} />
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
