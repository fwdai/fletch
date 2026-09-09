import type { AgentRecord } from "@desktop/api/types/agent";
import { spliceTranscript } from "@desktop/components/Composer/dictation/spliceTranscript";
import { useRef, useState } from "react";
import { Icon } from "../../components/Icon";
import { ProviderMark } from "../../components/ui";
import { DictationButton, useDictation } from "../../dictation";
import { isBusy, modelLabel } from "../../lib/agents";
import { autosize } from "../../lib/autosize";
import { ignore } from "../../lib/ignore";
import { useStore } from "../../store";

export function Composer({
  agent,
  onSend,
}: {
  agent: AgentRecord;
  /** Fires as the message goes out, so the screen can re-pin the log. */
  onSend?: () => void;
}) {
  const send = useStore((s) => s.send);
  const stop = useStore((s) => s.stop);
  const openSheet = useStore((s) => s.openSheet);
  const [text, setText] = useState("");
  const ta = useRef<HTMLTextAreaElement>(null);
  const busy = isBusy(agent);
  // The transcript lands once, appended to whatever was typed — the same join
  // rule as the desktop, so a dictated list item keeps its newline.
  const dictation = useDictation((spoken) => {
    setText((current) => spliceTranscript(current, spoken).text);
    requestAnimationFrame(() => autosize(ta.current));
  });

  const submit = () => {
    const value = text.trim();
    if (!value) return;
    setText("");
    requestAnimationFrame(() => autosize(ta.current));
    onSend?.();
    void send(agent.id, value).catch(ignore);
  };

  const placeholder =
    dictation.phase === "listening"
      ? "Listening…"
      : dictation.phase === "transcribing"
        ? "Transcribing…"
        : busy
          ? "Message agent · queued until it pauses"
          : "Message agent";

  return (
    <div className="composer">
      <div className="cmp">
        <textarea
          ref={ta}
          rows={1}
          placeholder={placeholder}
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
          {dictation.supported && (
            <DictationButton
              phase={dictation.phase}
              blocked={dictation.blocked}
              onToggle={() => void dictation.toggle()}
            />
          )}
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
