import { useEffect, useRef, useState } from "react";
import { useAppStore } from "../../store";
import { DEFAULT_PROVIDER_ID } from "../../data/providers";
import { Icon } from "../Icon";
import { Chip } from "../ui/Chip";
import { ModelPicker } from "./ModelPicker";

type ThinkingBudget = "low" | "medium" | "high";

interface Props {
  /** Initial provider id — defaults to claude. */
  defaultProvider?: string;
  /** When set, render a `from <branch>` chip and call `onChangeBase` on click. */
  baseBranch?: string;
  onChangeBase?: () => void;
  placeholder?: string;
  autoFocus?: boolean;
  disabled?: boolean;
  stopping?: boolean;
  /** Fired on Enter (without Shift) or send-button click. */
  onSend: (payload: { text: string; provider: string; thinking: ThinkingBudget }) => void;
  /** Fired when the composer is showing an active stop control. */
  onStop?: () => void;
}

export function Composer({
  defaultProvider = DEFAULT_PROVIDER_ID,
  baseBranch,
  onChangeBase,
  placeholder,
  autoFocus,
  disabled,
  stopping = false,
  onSend,
  onStop,
}: Props) {
  const features = useAppStore((s) => s.features);

  const [text, setText] = useState("");
  const [provider, setProvider] = useState(defaultProvider);
  const [thinking, setThinking] = useState<ThinkingBudget>("high");
  const ta = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    if (autoFocus) ta.current?.focus();
  }, [autoFocus]);

  function grow(el: HTMLTextAreaElement) {
    el.style.height = "auto";
    el.style.height = Math.min(el.scrollHeight, 240) + "px";
  }

  function send() {
    const trimmed = text.trim();
    if (stopping) {
      onStop?.();
      return;
    }
    if (!trimmed || disabled) return;
    onSend({ text: trimmed, provider, thinking });
    setText("");
    if (ta.current) ta.current.style.height = "auto";
  }

  function stop() {
    if (!stopping) return;
    onStop?.();
  }

  const sendDisabled = stopping ? !onStop : disabled || !text.trim();

  return (
    <div className="composer">
      <textarea
        ref={ta}
        className="composer-input"
        placeholder={placeholder || "Message agent · /commands · @ to attach · # for PRs"}
        value={text}
        rows={1}
        disabled={disabled}
        onChange={(e) => {
          setText(e.target.value);
          grow(e.target);
        }}
        onKeyDown={(e) => {
          if (e.key === "Enter" && !e.shiftKey) {
            e.preventDefault();
            send();
          }
        }}
      />
      <div className="composer-foot">
        <ModelPicker value={provider} onChange={setProvider} />
        {features.thinkingBudget && (
          <Chip
            tip="Thinking budget"
            onClick={() =>
              setThinking((t) =>
                t === "high" ? "medium" : t === "medium" ? "low" : "high",
              )
            }
          >
            <Icon name="sparkle" size={11} />
            <span style={{ textTransform: "capitalize" }}>{thinking}</span>
          </Chip>
        )}
        {features.autoEdit && (
          <Chip tip="Auto-approve writes">
            <Icon name="check" size={11} />
            <span>Auto-edit</span>
          </Chip>
        )}
        {baseBranch && (
          <Chip tip="Base branch" onClick={onChangeBase}>
            <Icon name="branch" size={11} />
            <span style={{ color: "var(--fg-2)" }}>from</span>
            <span style={{ fontFamily: "var(--font-mono)" }}>{baseBranch}</span>
          </Chip>
        )}
        <Chip tip="Attach">
          <Icon name="attach" size={11} />
        </Chip>
        <span style={{ flex: 1 }} />
        <button
          type="button"
          className={`send ${stopping ? "is-stop" : ""}`}
          disabled={sendDisabled}
          onClick={stopping ? stop : send}
          aria-label={stopping ? "Stop" : "Send"}
        >
          <Icon name={stopping ? "stop" : "arrowUp"} size={13} />
        </button>
      </div>
    </div>
  );
}
