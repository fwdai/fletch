import type { DictationAuthorization, DictationAvailability } from "@/api";
import { Icon } from "@/components/Icon";
import { IconButton } from "@/components/ui/IconButton";
import { Spinner } from "@/components/ui/Spinner";

/** What the user has to do outside the app. The backend answers a blocked start
 *  with the same guidance, so the tooltip reads identically whether we knew up
 *  front or found out on the attempt. */
const SETTINGS_HINT =
  "Enable Microphone and Speech Recognition for Fletch in System Settings → Privacy & Security";

/** The local engine never calls Apple's recognizer, so don't send the user
 *  looking for a permission it doesn't use. */
const MIC_HINT = "Enable Microphone for Fletch in System Settings → Privacy & Security";

/** The local engine ends the session itself once the user stops talking, so the
 *  tooltip has to say so — a mic that stops on its own otherwise reads as a bug,
 *  and the pause is the only thing the user has to do. */
const AUTO_STOP_HINT = "Listening… stops when you pause";

/** Authorization states the app can't recover from on its own — `restricted` is
 *  an MDM/parental lock, `denied` needs a trip to System Settings. */
const BLOCKED: DictationAuthorization[] = ["denied", "restricted"];

interface Props {
  /** null while the platform probe is in flight. */
  availability: DictationAvailability | null;
  listening: boolean;
  /** The previous session is still tearing down. The backend starts nothing in
   *  that window, so the control is held for the (sub-second) flush. */
  stopping: boolean;
  /** The local engine's model is running on what was just recorded — the same
   *  held window as `stopping`, but long enough to need saying so. */
  transcribing: boolean;
  /** Reason the last start failed, or null. */
  error: string | null;
  onToggle: () => void;
}

/** The composer's mic. Sits with the insert actions because dictation puts text
 *  into *this* message, like attaching a file does. */
export function DictationButton({
  availability,
  listening,
  stopping,
  transcribing,
  error,
  onToggle,
}: Props) {
  // Nothing at all until we know there's a native recognizer: a mic that can
  // only ever error is worse than no mic (Linux/Windows have none).
  if (!availability?.supported) return null;

  // The speech grant gates only Apple's recognizer; the local engine is blocked
  // by the microphone alone — and it's the only one that stops itself.
  const isLocal = availability.engine === "whisper";
  const blocked =
    BLOCKED.includes(availability.microphone) ||
    (!isLocal && BLOCKED.includes(availability.speech));
  const label = transcribing ? "Transcribing…" : listening ? "Stop dictation" : "Dictate";

  // The last failure outranks the standing permission hint, which outranks
  // what a live session is doing, which outranks the plain affordance. Blocked
  // still clicks through: permission can be granted between attempts, and the
  // failed start is what surfaces the reason.
  function tip() {
    if (error) return error;
    if (blocked) return isLocal ? MIC_HINT : SETTINGS_HINT;
    if (listening && isLocal) return AUTO_STOP_HINT;
    return label;
  }

  return (
    <IconButton
      className={`composer-action${listening ? " is-listening" : ""}`}
      tip={tip()}
      aria-label={label}
      aria-pressed={listening}
      disabled={stopping}
      onClick={onToggle}
    >
      {transcribing ? <Spinner size={15} /> : <Icon name={blocked ? "micOff" : "mic"} size={15} />}
    </IconButton>
  );
}
