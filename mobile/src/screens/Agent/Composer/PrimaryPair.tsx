import { ContourTrace } from "@desktop/components/Composer/PrimaryControl/ContourTrace";
import type { PrimaryState } from "@desktop/components/Composer/PrimaryControl/primaryState";
import { Icon } from "@desktop/components/Icon";
import type { ReactNode } from "react";

/** The disc, in px: 40 visual inside a 44 hit area, r20 — a circle. */
const DISC = 40;

const LABELS: Record<PrimaryState, string> = {
  empty: "Dictate",
  draft: "Send",
  listening: "Done — stop and transcribe",
  transcribing: "Transcribing",
  running: "Stop agent",
  error: "Couldn't transcribe — tap to retry",
  unavailable: "Dictation is off",
};

/** The primary disc plus the mic that steps aside. In `empty` the mic sits on
 *  the disc; in `draft` it slides 50 pt left into its own smaller neutral disc
 *  and the primary fills accent with the arrow. Accent is always send; neutral
 *  is always "dictate more". The disc never moves or resizes — glyphs rotate
 *  through it. */
export function PrimaryPair({
  state,
  hasMic,
  armed,
  onPrimary,
  onMic,
}: {
  state: PrimaryState;
  /** The host can transcribe and this webview has a microphone. Without it the
   *  mic is never offered and the empty disc is a plain, inert send arrow. */
  hasMic: boolean;
  /** False for a moment after send: the stop is inert (see `SEND_ARM_MS`). */
  armed: boolean;
  onPrimary: () => void;
  onMic: () => void;
}) {
  const is = (s: PrimaryState) => state === s;
  const micHome = hasMic && is("empty");
  const micAside = hasMic && is("draft");
  const noMicEmpty = !hasMic && is("empty");
  const inert = is("transcribing") || noMicEmpty;

  return (
    <span className="mpc-wrap">
      <button
        type="button"
        className={`m-sec${micAside ? " aside" : ""}${micHome || micAside ? "" : " gone"}`}
        aria-label={micAside ? "Dictate more" : "Dictate"}
        tabIndex={micHome || micAside ? 0 : -1}
        disabled={!(micHome || micAside)}
        onClick={onMic}
      >
        <span className="circ">
          <Icon name="mic" size={20} />
        </span>
      </button>
      <button
        type="button"
        className={`mpc s-${state}${armed ? "" : " unarmed"}${hasMic ? "" : " no-mic"}`}
        aria-label={noMicEmpty ? "Send" : LABELS[state]}
        disabled={inert}
        onClick={onPrimary}
      >
        <span className="disc" />
        <ContourTrace active={is("running")} size={DISC} radius={DISC / 2} lapMs={1800} />
        <Glyph on={is("error")}>
          <Icon name="mic" size={20} />
        </Glyph>
        <Glyph on={is("draft") || noMicEmpty}>
          <Icon name="arrowUp" size={20} strokeWidth={2} />
        </Glyph>
        <Glyph on={is("listening")}>
          <Icon name="check" size={20} strokeWidth={2.2} />
        </Glyph>
        <Glyph on={is("transcribing")}>
          <span className="mpc-spin" />
        </Glyph>
        <Glyph on={is("running")}>
          <span className="mpc-stop" />
        </Glyph>
        <Glyph on={is("unavailable")}>
          <Icon name="micOff" size={20} />
        </Glyph>
      </button>
    </span>
  );
}

/** One of the disc's glyphs; the one that is `on` scales and rotates in. */
function Glyph({ on, children }: { on: boolean; children: ReactNode }) {
  return <span className={`g${on ? " on" : ""}`}>{children}</span>;
}
