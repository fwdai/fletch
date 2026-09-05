import type { DictationAuthorization, DictationAvailability } from "@/api";
import { Icon } from "@/components/Icon";
import { IconButton } from "@/components/ui/IconButton";

/** What the user has to do outside the app. The backend answers a blocked start
 *  with the same guidance, so the tooltip reads identically whether we knew up
 *  front or found out on the attempt. */
const SETTINGS_HINT =
  "Enable Microphone and Speech Recognition for Fletch in System Settings → Privacy & Security";

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
  /** Reason the last start failed, or null. */
  error: string | null;
  onToggle: () => void;
}

/** The composer's mic. Sits with the insert actions because dictation puts text
 *  into *this* message, like attaching a file does. */
export function DictationButton({ availability, listening, stopping, error, onToggle }: Props) {
  // Nothing at all until we know there's a native recognizer: a mic that can
  // only ever error is worse than no mic (Linux/Windows have none).
  if (!availability?.supported) return null;

  const blocked =
    BLOCKED.includes(availability.speech) || BLOCKED.includes(availability.microphone);
  const label = listening ? "Stop dictation" : "Dictate";

  // The last failure outranks the standing permission hint, which outranks the
  // plain affordance. Blocked still clicks through: permission can be granted
  // between attempts, and the failed start is what surfaces the reason.
  function tip() {
    if (error) return error;
    if (blocked) return SETTINGS_HINT;
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
      <Icon name={blocked ? "micOff" : "mic"} size={15} />
    </IconButton>
  );
}
