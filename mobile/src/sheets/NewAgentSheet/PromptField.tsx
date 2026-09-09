import { spliceTranscript } from "@desktop/components/Composer/dictation/spliceTranscript";
import { primaryState } from "@desktop/components/Composer/PrimaryControl/primaryState";
import { type ReactNode, useEffect, useRef } from "react";
import { Icon } from "../../components/Icon";
import { useDictation, VoiceRow } from "../../dictation";
import { autosize } from "../../lib/autosize";

/** The heights the field grows between, in px. Mirrors `.na-prompt textarea`
 *  in screens.css, which sets the resting one. */
const MIN_PX = 120;
const MAX_PX = 260;

/** The first prompt for a new agent: the field, and the row of pickers along
 *  its foot. The mic sits at the right of that row and, while it is open, the
 *  pickers give way to the waveform — the same dictation the chat composer
 *  offers, in the shape this box has room for. */
export function PromptField({
  value,
  onChange,
  onDictating,
  children,
}: {
  value: string;
  onChange: (next: string) => void;
  /** Fires as a session opens and closes, so the sheet can hold its Start
   *  button until the words have landed in the draft. */
  onDictating: (live: boolean) => void;
  /** The pickers under the field (the runner chip). Hidden while the waveform
   *  has the row. */
  children: ReactNode;
}) {
  const ta = useRef<HTMLTextAreaElement>(null);
  // Read in the dictation callback, which lands whenever the host answers —
  // against whatever the box holds by then, not what it held at the tap.
  const valueRef = useRef(value);
  valueRef.current = value;

  // The transcript arrives once, at the end (whisper has no partials), and is
  // appended with the same join rule as the desktop so a dictated list item
  // keeps its newline.
  const dictation = useDictation((spoken) => {
    onChange(spliceTranscript(valueRef.current, spoken).text);
    requestAnimationFrame(() => autosize(ta.current, MIN_PX, MAX_PX));
  });

  const state = primaryState({
    sttError: dictation.error !== null,
    dictation: dictation.phase,
    agentRunning: false,
    hasDraft: value.trim().length > 0,
    micDenied: dictation.blocked,
  });
  const listening = state === "listening";
  const voice = listening || state === "transcribing";

  useEffect(() => {
    onDictating(dictation.phase !== "idle");
  }, [dictation.phase, onDictating]);

  return (
    <>
      <div className={`na-prompt${listening ? " is-listening" : ""}`}>
        <textarea
          ref={ta}
          rows={4}
          placeholder={
            dictation.supported
              ? "Describe the task, or tap the mic — this becomes the agent's first message."
              : "Describe the task — this becomes the agent's first message."
          }
          value={value}
          onChange={(e) => {
            onChange(e.target.value);
            autosize(e.currentTarget, MIN_PX, MAX_PX);
          }}
        />
        <div className="na-tools">
          {voice ? (
            <VoiceRow
              className="na-voice"
              state={state}
              levels={dictation.levels}
              startedAt={dictation.startedAt}
              onCancel={dictation.cancel}
            />
          ) : (
            <>
              {children}
              <span className="grow" />
            </>
          )}
          {dictation.supported && (
            <button
              type="button"
              className={`na-mic${listening ? " on" : ""}`}
              aria-label={
                listening
                  ? "Done — stop and transcribe"
                  : dictation.blocked
                    ? "Dictation is off"
                    : "Dictate"
              }
              // Nothing to do while the host transcribes; the row says so.
              disabled={state === "transcribing"}
              onClick={() => void dictation.toggle()}
            >
              {listening ? (
                <Icon name="check" size={16} sw={2.2} />
              ) : (
                <Icon name={dictation.blocked ? "micOff" : "mic"} size={16} />
              )}
            </button>
          )}
        </div>
      </div>
      {dictation.error && <div className="err na-err">{dictation.error}</div>}
    </>
  );
}
