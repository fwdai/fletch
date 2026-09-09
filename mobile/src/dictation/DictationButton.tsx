import { Icon } from "../components/Icon";
import type { DictationPhase } from "./useDictation";

/** The composer's mic. Lives in the bar next to send, because dictation puts
 *  text into *this* message. Tap to start, tap again to stop; the spinner is
 *  the wait for the Mac to run the model on what was said. */
export function DictationButton({
  phase,
  blocked,
  onToggle,
}: {
  phase: DictationPhase;
  /** The Mac can't transcribe right now. Still tappable: the tap surfaces why. */
  blocked: boolean;
  onToggle: () => void;
}) {
  const listening = phase === "listening";
  const busy = phase === "starting" || phase === "transcribing";
  const label = listening
    ? "Stop dictation"
    : phase === "transcribing"
      ? "Transcribing"
      : "Dictate";
  return (
    <button
      type="button"
      className={`micbtn${listening ? " listening" : ""}${blocked ? " off" : ""}`}
      onClick={onToggle}
      disabled={busy}
      aria-label={label}
      aria-pressed={listening}
    >
      {phase === "transcribing" ? (
        <Icon name="refresh" size={16} className="spin" />
      ) : (
        <Icon name={blocked ? "micOff" : "mic"} size={16} />
      )}
    </button>
  );
}
