import type { AgentRecord } from "@desktop/api/types/agent";
import {
  type FreshSpan,
  spliceTranscript,
} from "@desktop/components/Composer/dictation/spliceTranscript";
import { InterimGhost } from "@desktop/components/Composer/InterimGhost";
import { primaryState } from "@desktop/components/Composer/PrimaryControl/primaryState";
import { Icon } from "@desktop/components/Icon";
import { useEffect, useRef, useState } from "react";
import { ProviderMark } from "../../../components/ui";
import { useDictation, VoiceRow } from "../../../dictation";
import { isBusy, modelLabel } from "../../../lib/agents";
import { autosize } from "../../../lib/autosize";
import { ignore } from "../../../lib/ignore";
import { useStore } from "../../../store";
import { PrimaryPair } from "./PrimaryPair";

/** Stop ignores taps this long after send, so a double-tap can't kill the run
 *  it just started. Taps are swallowed, not queued. */
const SEND_ARM_MS = 450;

/** How long a just-committed span is marked. A hair longer than its CSS wash,
 *  which fades over 1.1 s. */
const FRESH_TTL_MS = 1200;

/** Field height cap before it scrolls internally: the field follows the text
 *  up to 40% of the visible viewport, so a long draft stays readable while the
 *  log keeps most of the screen. Read per call, since the keyboard changes the
 *  viewport. Mirrors the `40dvh` max-height in agent.css. */
const fieldCap = () => Math.round(window.innerHeight * 0.4);

/** The phone's composer: the field, and a footer whose primary control is one
 *  44 pt disc that morphs in place — mic → send → done → stop — with the mic
 *  stepping aside as a smaller neutral disc once there is a draft. Same state
 *  machine as the desktop pill (`primaryState`), different physics: no split
 *  segments a thumb could miss, every live state has a visible exit, and the
 *  return key inserts a newline — send is only ever a deliberate tap. */
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
  // Read inside the dictation callback, which arrives whenever the host
  // answers, against whatever the box holds by then.
  const textRef = useRef(text);
  textRef.current = text;
  const ta = useRef<HTMLTextAreaElement>(null);
  const [fresh, setFresh] = useState<FreshSpan | null>(null);
  const freshTimer = useRef<number | null>(null);
  const [armed, setArmed] = useState(true);
  const armTimer = useRef<number | null>(null);
  const busy = isBusy(agent);

  // The transcript lands once, appended to whatever was typed — the same join
  // rule as the desktop, so a dictated list item keeps its newline. The new
  // span washes for a moment so the eye can find it.
  const dictation = useDictation((spoken) => {
    const { text: next, start, caret } = spliceTranscript(textRef.current, spoken);
    setText(next);
    setFresh({ start, end: caret });
    if (freshTimer.current !== null) window.clearTimeout(freshTimer.current);
    freshTimer.current = window.setTimeout(() => {
      freshTimer.current = null;
      setFresh(null);
    }, FRESH_TTL_MS);
    requestAnimationFrame(() => autosize(ta.current, 0, fieldCap()));
  });

  useEffect(
    () => () => {
      if (freshTimer.current !== null) window.clearTimeout(freshTimer.current);
      if (armTimer.current !== null) window.clearTimeout(armTimer.current);
    },
    [],
  );

  const hasMic = dictation.supported;
  const state = primaryState({
    sttError: dictation.error !== null,
    dictation: dictation.phase,
    agentRunning: busy,
    hasDraft: text.trim().length > 0,
    micDenied: dictation.blocked,
  });
  const listening = state === "listening";
  const voice = listening || state === "transcribing";

  // The banner keeps its last line while it collapses, so the text doesn't
  // vanish a frame before the space does.
  const lastError = useRef("");
  if (dictation.error) lastError.current = dictation.error;

  /** Send the draft. Only from `draft`: while the agent works the draft waits,
   *  and nothing is sent by voice alone. */
  const submit = () => {
    const value = text.trim();
    if (!value || busy || dictation.phase !== "idle") return;
    setText("");
    requestAnimationFrame(() => autosize(ta.current, 0, fieldCap()));
    setArmed(false);
    if (armTimer.current !== null) window.clearTimeout(armTimer.current);
    armTimer.current = window.setTimeout(() => {
      armTimer.current = null;
      setArmed(true);
    }, SEND_ARM_MS);
    onSend?.();
    void send(agent.id, value).catch(ignore);
  };

  /** A tap on the disc — what it does is what the disc shows. */
  const onPrimary = () => {
    switch (state) {
      case "empty":
        if (hasMic) void dictation.start();
        break;
      case "error":
      case "unavailable":
        // Retry; or, blocked, surface why in the banner.
        void dictation.start();
        break;
      case "draft":
        submit();
        break;
      case "listening":
        void dictation.toggle();
        break;
      case "running":
        if (armed) void stop(agent.id).catch(ignore);
        break;
      default:
        break;
    }
  };

  const placeholder =
    state === "unavailable"
      ? "Type a message · dictation is off"
      : busy
        ? "Agent is working · the draft waits"
        : hasMic
          ? "Type, or tap the mic to dictate"
          : "Message agent";

  return (
    <div className="composer">
      <div
        className={`cmp${listening ? " is-listening" : ""}${dictation.error ? " is-error" : ""}`}
      >
        <div className="cmp-field">
          <textarea
            ref={ta}
            rows={1}
            placeholder={placeholder}
            value={text}
            onChange={(e) => {
              setText(e.target.value);
              autosize(e.currentTarget, 0, fieldCap());
            }}
          />
          {(listening || fresh !== null) && (
            <div className="cmp-ghost-clip">
              <InterimGhost
                text={text}
                caret={text.length}
                interim=""
                listening={listening}
                fresh={fresh}
              />
            </div>
          )}
        </div>
        <div className={`cmp-banner${dictation.error ? " on" : ""}`} role="alert">
          <div>
            <p>
              <Icon name="refresh" size={12} />
              {lastError.current}
            </p>
          </div>
        </div>
        <div className="cmp-foot">
          <div className="cmp-rows">
            <div className={`cmp-row actions${voice ? " off" : ""}`}>
              <button
                type="button"
                className="mdl"
                tabIndex={voice ? -1 : 0}
                onClick={() => openSheet("modelPicker", { agentId: agent.id })}
              >
                <ProviderMark id={agent.provider} />
                <span>{modelLabel(agent.model)}</span>
                <span className="sep">·</span>
                <span>{agent.effort ?? "default"}</span>
                <Icon name="chevD" size={12} style={{ color: "var(--fg-3)" }} />
              </button>
              <span className="grow" />
            </div>
            <VoiceRow
              className={`cmp-row voice${voice ? "" : " off"}`}
              state={state}
              levels={dictation.levels}
              startedAt={dictation.startedAt}
              onCancel={dictation.cancel}
            />
          </div>
          <PrimaryPair
            state={state}
            hasMic={hasMic}
            armed={armed}
            onPrimary={onPrimary}
            onMic={() => void dictation.start()}
          />
        </div>
      </div>
    </div>
  );
}
